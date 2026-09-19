"""Server-side, application-only Compose rollout. No build, migration, or database restore."""

import argparse
import copy
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import threading
import time
import urllib.request


def digest(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def inspect(name):
    return json.loads(subprocess.check_output(["docker", "inspect", name]))[0]


def atomic_copy(source, target):
    pending = temporary_sibling(target)
    try:
        shutil.copy2(source, pending)
        stat = source.stat()
        os.chown(pending, stat.st_uid, stat.st_gid)
        os.replace(pending, target)
    finally:
        pending.unlink(missing_ok=True)


def temporary_sibling(target):
    descriptor, name = tempfile.mkstemp(prefix=".cpr-pending-", suffix=".yaml", dir=target.parent)
    os.close(descriptor)
    return Path(name)


def changed_image(document, service, image):
    result = copy.deepcopy(document)
    result["services"][service]["image"] = image
    return result


def healthy(profile):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(profile["health_url"], timeout=1) as response:
            return response.status == profile.get("health_status", 204)
    except Exception:
        return False


class Rollout:
    def __init__(self, request, archive):
        self.profile = request["profile"]
        self.selected = request["selected"]
        self.proof = self.selected["proof"]
        self.old_image = request["expected_old_image"]
        self.archive = Path(archive)
        self.compose = Path(self.profile["compose_file"])
        self.backup = self.compose.parent / ".deployment-backups" / (
            time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()) + "-" + str(os.getpid())
        )
        self.samples = []
        self.stop_monitor = threading.Event()
        self.pending = None

    def compose_run(self, *args, file=None):
        return subprocess.run(
            ["docker", "compose", "--project-name", self.project,
             "--project-directory", str(self.compose.parent),
             "-f", str(file or self.compose), *args],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=240,
        )

    def up(self, log_name):
        result = self.compose_run(
            "up", "-d", "--no-deps", "--no-build", "--pull", "never", self.profile["service"]
        )
        (self.backup / log_name).write_bytes(result.stdout)
        if result.returncode:
            raise RuntimeError("Compose application update failed")

    def ready(self, image):
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            try:
                app = inspect(self.profile["container"])
                if app["State"].get("Status") in {"exited", "dead"}:
                    raise RuntimeError("Application exited before becoming ready")
                if app["Image"] == image and app["State"]["Running"] and healthy(self.profile):
                    return app
            except (OSError, subprocess.SubprocessError):
                pass
            time.sleep(0.5)
        raise RuntimeError("Application did not become ready")

    def monitor(self):
        while not self.stop_monitor.is_set():
            self.samples.append((time.monotonic(), healthy(self.profile)))
            self.stop_monitor.wait(0.2)

    def prepare(self):
        import yaml

        if self.compose.is_symlink() or not self.compose.is_absolute():
            raise RuntimeError("Use a regular absolute Compose file")
        self.old = inspect(self.profile["container"])
        if (self.old["Image"] != self.old_image or not self.old["State"]["Running"]
                or not healthy(self.profile)):
            raise RuntimeError("Current deployment changed or is unhealthy")
        labels = self.old["Config"].get("Labels", {})
        if labels.get("com.docker.compose.service") != self.profile["service"]:
            raise RuntimeError("Container does not belong to the selected Compose service")
        self.project = labels.get("com.docker.compose.project")
        if not self.project:
            raise RuntimeError("Current container lacks a Compose project identity")
        files = labels.get("com.docker.compose.project.config_files", "")
        if files != str(self.compose):
            raise RuntimeError("Fast deployment requires the current single-file Compose configuration")
        if (self.old["Config"].get("StopTimeout") or 75) > 150:
            raise RuntimeError("Drain budget exceeds the fast deployment timeout")
        if self.profile["container"] in self.profile["protected_containers"]:
            raise RuntimeError("Invalid protected container list")
        self.protected = {name: inspect(name) for name in self.profile["protected_containers"]}
        self.configs = {Path(path): digest(path) for path in self.profile.get("config_files", [])}
        if digest(self.archive) != self.proof["archive_sha256"]:
            raise RuntimeError("Transferred archive checksum mismatch")
        loaded = subprocess.run(["docker", "image", "load", "--input", str(self.archive)],
                                capture_output=True, timeout=180)
        if loaded.returncode:
            raise RuntimeError("Image import failed")
        image = inspect(self.proof["image_tag"])
        labels = image["Config"].get("Labels", {}) or {}
        if (labels.get("org.opencontainers.image.revision") != self.proof["source_commit"]
                or labels.get("org.opencontainers.image.version") != self.proof["version"]
                or image["Architecture"] != "amd64" or image["Os"] != "linux"):
            raise RuntimeError("Imported image identity mismatch")
        self.new_image = image["Id"]
        # Docker's classic and containerd stores can expose different archive image IDs.
        # The transferred bytes are pinned above; deploy the destination's immutable ID.
        self.compose_digest = digest(self.compose)
        self.backup.mkdir(parents=True, mode=0o700)
        shutil.copy2(self.compose, self.backup / "compose.yaml")
        for index, path in enumerate(self.configs):
            shutil.copy2(path, self.backup / f"config-{index}")
        if digest(self.backup / "compose.yaml") != self.compose_digest:
            raise RuntimeError("Compose configuration changed while creating the backup")
        document = yaml.safe_load((self.backup / "compose.yaml").read_text())
        updated = changed_image(document, self.profile["service"], self.new_image)
        self.pending = temporary_sibling(self.compose)
        self.pending.write_text(yaml.safe_dump(updated, sort_keys=False))
        shutil.copystat(self.compose, self.pending)
        stat = self.compose.stat()
        os.chown(self.pending, stat.st_uid, stat.st_gid)
        if yaml.safe_load(self.pending.read_text()) != updated:
            raise RuntimeError("Compose round-trip changed configuration")
        result = self.compose_run("config", "--quiet", file=self.pending)
        if result.returncode:
            raise RuntimeError("Candidate Compose configuration is invalid")
        rollback = self.backup / "rollback.yaml"
        rollback.write_text(yaml.safe_dump(
            changed_image(document, self.profile["service"], self.old_image), sort_keys=False))
        shutil.copystat(self.compose, rollback)
        os.chown(rollback, stat.st_uid, stat.st_gid)
        (self.backup / "selected.json").write_text(json.dumps(self.selected, indent=2))

    def check_unchanged(self):
        if inspect(self.profile["container"])["Id"] != self.old["Id"]:
            raise RuntimeError("Another deployment changed the running container")
        if digest(self.compose) != self.compose_digest:
            raise RuntimeError("Compose configuration changed during preparation")
        if any(digest(path) != expected for path, expected in self.configs.items()):
            raise RuntimeError("Application configuration changed during preparation")

    def switch(self):
        self.check_unchanged()
        switched = False
        result = {"status": "not_switched", "target_commit": self.selected["target_commit"],
                  "tested_commit": self.proof["source_commit"], "image": self.new_image}
        self.samples.append((time.monotonic(), True))
        monitor = threading.Thread(target=self.monitor)
        monitor.start()
        started = time.monotonic()
        try:
            os.replace(self.pending, self.compose)
            switched = True
            print("Prepared image and backups; switching application only.", flush=True)
            self.up("upgrade.log")
            current = self.ready(self.new_image)
            if current["RestartCount"]:
                raise RuntimeError("New container restarted unexpectedly")
            for name, previous in self.protected.items():
                after = inspect(name)
                if (after["Id"] != previous["Id"]
                        or after["State"]["StartedAt"] != previous["State"]["StartedAt"]):
                    raise RuntimeError("A protected service changed during deployment")
            if any(digest(path) != expected for path, expected in self.configs.items()):
                raise RuntimeError("Application configuration changed")
            for _ in range(5):
                time.sleep(1)
                if not healthy(self.profile):
                    raise RuntimeError("Post-switch health check failed")
            result["status"] = "deployed"
        except BaseException as error:
            result["failure_type"] = type(error).__name__
            import traceback
            (self.backup / "failure.txt").write_text(traceback.format_exc())
            if switched:
                try:
                    atomic_copy(self.backup / "rollback.yaml", self.compose)
                    self.up("rollback.log")
                    self.ready(self.old_image)
                    result["status"] = "rolled_back"
                except BaseException:
                    result["status"] = "rollback_failed"
            else:
                raise
        finally:
            self.stop_monitor.set()
            monitor.join()
        result["verification_seconds"] = round(time.monotonic() - started, 3)
        result["health_gap_seconds_upper_estimate"] = health_gap(self.samples)
        (self.backup / "result.json").write_text(json.dumps(result, indent=2))
        (self.backup / "health-samples.json").write_text(json.dumps(self.samples))
        if result["status"] == "deployed":
            record = self.compose.parent / ".cpr-release.json"
            pending = record.with_suffix(".pending")
            pending.write_text(json.dumps({**result, "proof": self.proof}, indent=2))
            os.replace(pending, record)
        print(json.dumps(result), flush=True)
        return 0 if result["status"] == "deployed" else 1

    def run(self):
        # Shared with earlier deployments; an independent lock per worktree is unsafe.
        with self.compose.with_name(".cpr-deployment.lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            try:
                self.prepare()
                return self.switch()
            finally:
                if self.pending is not None:
                    self.pending.unlink(missing_ok=True)


def health_gap(samples):
    previous = None
    start = None
    longest = 0.0
    for when, ok in samples:
        if ok:
            if start is not None:
                longest = max(longest, when - start)
                start = None
            previous = when
        elif start is None:
            start = previous if previous is not None else when
    if start is not None and samples:
        longest = max(longest, samples[-1][0] - start)
    return round(longest, 3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    if not args.apply:
        parser.error("Server mutation requires --apply")
    os.umask(0o077)
    def interrupted(signum, frame):
        # A dropped SSH session should enter rollback, not abandon the cutover.
        signal.signal(signal.SIGHUP, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError("Deployment interrupted")

    signal.signal(signal.SIGHUP, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    try:
        return Rollout(json.loads(args.request.read_text()), args.archive).run()
    except Exception as error:
        # Keep private config values out of terminal output.
        print("Deployment stopped; inspect the private deployment result: " + type(error).__name__)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
