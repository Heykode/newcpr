"""Opt-in Docker integration test. Uses an isolated project and loopback port."""

import gzip
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
import uuid

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import rollout


@unittest.skipUnless(os.environ.get("CPR_ROLLOUT_DOCKER_TEST") == "1", "explicit Docker opt-in required")
class DockerRolloutTests(unittest.TestCase):
    def test_real_switch_and_failed_image_rollback(self):
        name = "cpr-rollout-smoke-" + uuid.uuid4().hex[:10]
        images = [name + ":" + version for version in ("old", "new", "bad")]
        base = os.environ.get("CPR_ROLLOUT_TEST_BASE", "node:24-bookworm-slim")

        def command(*args):
            return subprocess.check_output(args, stderr=subprocess.STDOUT, timeout=180).decode().strip()

        def snapshot():
            ids = command("docker", "ps", "-q").split()
            if not ids:
                return {}
            result = subprocess.run([
                "docker", "inspect", "--format", "{{.Id}} {{.State.StartedAt}}", *ids,
            ], capture_output=True, text=True, timeout=30)
            # Short-lived unrelated jobs can disappear between list and inspect.
            return dict(line.split() for line in result.stdout.splitlines() if len(line.split()) == 2)

        before = snapshot()
        with tempfile.TemporaryDirectory(prefix="cpr-docker-test-") as temporary:
            directory = Path(temporary)
            compose = directory / "compose.yaml"
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            document = {
                "name": name, "services": {
                    "app": {"image": images[0], "container_name": name + "-app",
                            "ports": [f"127.0.0.1:{port}:8080"], "stop_grace_period": "2s"},
                    "sentinel": {"image": base, "container_name": name + "-sentinel",
                                 "command": ["node", "-e", "setInterval(()=>{},1000)"]},
                },
            }
            compose.write_text(yaml.safe_dump(document))
            profile = {
                "compose_file": str(compose), "container": name + "-app", "service": "app",
                "health_url": f"http://127.0.0.1:{port}/healthz", "health_status": 204,
                "protected_containers": [name + "-sentinel"],
            }
            try:
                for index, tag in enumerate(images):
                    source = str(index + 1) * 40
                    script = (
                        "require('http').createServer((q,s)=>{s.writeHead(204);s.end()})"
                        ".listen(8080,'0.0.0.0')"
                    ) if index < 2 else "process.exit(1)"
                    (directory / "Dockerfile").write_text(
                        f"FROM {base}\n"
                        f"LABEL org.opencontainers.image.revision={source}\n"
                        f"LABEL org.opencontainers.image.version=1.0.{index}\n"
                        "CMD " + json.dumps(["node", "-e", script]) + "\n"
                    )
                    command("docker", "build", "--pull=false", "-t", tag, str(directory))
                command("docker", "compose", "-f", str(compose), "up", "-d", "--pull", "never")
                deadline = time.monotonic() + 20
                while not rollout.healthy(profile):
                    if time.monotonic() > deadline:
                        self.fail("Synthetic service did not start")
                    time.sleep(0.2)
                sentinel = rollout.inspect(name + "-sentinel")
                for index, expected_status in ((1, "deployed"), (2, "rolled_back")):
                    archive = directory / f"image-{index}.tar.gz"
                    tar = directory / "image.tar"
                    with tar.open("wb") as output:
                        subprocess.run(["docker", "image", "save", images[index]],
                                       stdout=output, check=True, timeout=120)
                    with tar.open("rb") as source_file, gzip.open(archive, "wb", compresslevel=1) as output:
                        shutil.copyfileobj(source_file, output)
                    tar.unlink()
                    old = rollout.inspect(name + "-app")["Image"]
                    request = {
                        "profile": profile, "expected_old_image": old,
                        "selected": {"target_commit": "a" * 40, "proof": {
                            "image_tag": images[index], "archive_sha256": rollout.digest(archive),
                            "source_commit": str(index + 1) * 40, "version": f"1.0.{index}",
                        }},
                    }
                    request_file = directory / "request.json"
                    request_file.write_text(json.dumps(request))
                    result = subprocess.run([
                        sys.executable, str(Path(rollout.__file__)), "--request", str(request_file),
                        "--archive", str(archive), "--apply",
                    ], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=180)
                    self.assertEqual(result.returncode, 0 if index == 1 else 1, result.stdout)
                    results = sorted((directory / ".deployment-backups").glob("*/result.json"))
                    report = json.loads(results[-1].read_text())
                    self.assertEqual(report["status"], expected_status, result.stdout)
                    print(json.dumps({
                        "case": expected_status,
                        "health_gap_seconds_upper_estimate": report["health_gap_seconds_upper_estimate"],
                    }), flush=True)
                    self.assertTrue(rollout.healthy(profile))
                    if index == 2:
                        self.assertEqual(rollout.inspect(name + "-app")["Image"], old)
                    after = rollout.inspect(name + "-sentinel")
                    self.assertEqual(after["Id"], sentinel["Id"])
                    self.assertEqual(after["State"]["StartedAt"], sentinel["State"]["StartedAt"])
            finally:
                # Cleanup is restricted to this randomly named test project.
                subprocess.run(["docker", "compose", "-f", str(compose), "down", "--remove-orphans"],
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=60)
                subprocess.run(["docker", "image", "rm", *images],
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=60)
            after = snapshot()
            self.assertTrue(all(after.get(key) == value for key, value in before.items()),
                            "An existing service changed during the isolated test")


if __name__ == "__main__":
    unittest.main()
