import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "release/cargo-cache/cargo-cache.sh"


class CargoSnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.cache = self.root / "cache"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        docker = self.bin / "docker"
        docker.write_text("""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with open(os.environ["CALLS"], "a") as stream:
    stream.write(json.dumps(args) + "\\n")
target = args[args.index("--target") + 1]
if target == os.environ.get("FAIL_TARGET"):
    sys.exit(23)
if target == "snapshot":
    sys.stdout.buffer.write(Path(os.environ["ARCHIVE"]).read_bytes())
""")
        docker.chmod(0o755)
        self.calls = self.root / "calls"
        self.archive = self.root / "archive.tar"
        self.output = self.root / "output"
        self.env = {**os.environ, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
                    "CALLS": str(self.calls), "ARCHIVE": str(self.archive),
                    "GITHUB_OUTPUT": str(self.output)}

    def seed(self):
        self.cache.mkdir(exist_ok=True)
        for name, path in (("registry", "cache/source/old.crate"), ("target", "amd64/release/deps/old.rlib"), ("git", "db/old")):
            (self.cache / (name + ".tar.gz")).write_bytes(self.compressed(path, b"old-cache"))

    def compressed(self, path, body):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            info = tarfile.TarInfo(path)
            info.size = len(body)
            archive.addfile(info, io.BytesIO(body))
        return buffer.getvalue()

    def make_archive(self, valid=True):
        with tarfile.open(self.archive, "w") as archive:
            directory = tarfile.TarInfo("cache")
            directory.type = tarfile.DIRTYPE
            archive.addfile(directory)
            for name, path in (("registry", "cache/source/new.crate"), ("target", "amd64/release/deps/new.rlib" if valid else "invalid"), ("git", "db/new")):
                body = self.compressed(path, b"new")
                info = tarfile.TarInfo("cache/" + name + ".tar.gz")
                info.size = len(body)
                archive.addfile(info, io.BytesIO(body))

    def run_script(self, operation):
        return subprocess.run(["bash", str(SCRIPT), operation, "fixture", str(self.cache), "amd64"],
                              env=self.env, capture_output=True, text=True)

    def test_empty_restore_does_not_call_docker(self):
        result = self.run_script("restore")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.calls.exists())
        self.assertEqual(self.output.read_text(), "usable=false\n")

    def test_restore_uses_existing_valid_snapshot(self):
        self.seed()
        result = self.run_script("restore")
        self.assertEqual(result.returncode, 0, result.stderr)
        args = json.loads(self.calls.read_text())
        self.assertIn("type=cacheonly", args)
        self.assertEqual(args[-1], str(self.cache))
        self.assertEqual(self.output.read_text(), "usable=true\n")

    def test_restore_ignores_legacy_expanded_snapshot(self):
        for name in ("registry", "git", "target"):
            (self.cache / name).mkdir(parents=True)
        result = self.run_script("restore")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.calls.exists())
        self.assertEqual(self.output.read_text(), "usable=false\n")

    def test_restore_ignores_corrupt_or_wrong_architecture_snapshot(self):
        for payload in (b"not-gzip", self.compressed("arm64/release/deps/a.rlib", b"other-architecture")):
            self.seed()
            (self.cache / "target.tar.gz").write_bytes(payload)
            self.output.unlink(missing_ok=True)
            result = self.run_script("restore")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(self.calls.exists())
            self.assertEqual(self.output.read_text(), "usable=false\n")

    def test_tar_export_replaces_old_snapshot_after_validation(self):
        self.seed()
        self.make_archive()
        result = self.run_script("snapshot")
        self.assertEqual(result.returncode, 0, result.stderr)
        with tarfile.open(self.cache / "registry.tar.gz", "r:gz") as archive:
            self.assertEqual(archive.extractfile("cache/source/new.crate").read(), b"new")
            self.assertNotIn("cache/source/old.crate", archive.getnames())
        self.assertFalse((self.cache / "registry").exists())
        self.assertIn("type=tar,dest=-", json.loads(self.calls.read_text()))
        self.assertEqual(list(self.root.glob("cache.*")), [])

    def test_failed_or_incomplete_export_preserves_previous_snapshot(self):
        self.seed()
        self.make_archive(valid=False)
        incomplete = self.run_script("snapshot")
        self.assertEqual(incomplete.returncode, 0, incomplete.stderr)
        self.assertEqual(self.output.read_text(), "usable=false\n")
        self.env["FAIL_TARGET"] = "snapshot"
        failed = self.run_script("snapshot")
        self.assertNotEqual(failed.returncode, 0)
        with tarfile.open(self.cache / "registry.tar.gz", "r:gz") as archive:
            self.assertEqual(archive.extractfile("cache/source/old.crate").read(), b"old-cache")
        self.assertEqual(list(self.root.glob("cache.*")), [])

    def test_source_build_order_and_failure_do_not_export_stale_cache(self):
        self.seed()
        self.make_archive()
        command = ["bash", str(ROOT / "release/build_source_image.sh"), "fixture",
                   str(self.cache), "amd64", "fixture-image:test"]
        result = subprocess.run(command, env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [json.loads(line) for line in self.calls.read_text().splitlines()]
        self.assertEqual([args[args.index("--target") + 1] for args in calls],
                         ["restore", "runtime", "snapshot"])
        self.assertIn("linux/amd64", calls[1])
        self.assertTrue(any(arg.startswith("CPR_GIT_SHA=") for arg in calls[1]))
        self.calls.unlink()
        self.env["FAIL_TARGET"] = "runtime"
        result = subprocess.run(command, env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 23)
        calls = [json.loads(line) for line in self.calls.read_text().splitlines()]
        self.assertEqual([args[args.index("--target") + 1] for args in calls], ["restore", "runtime"])


if __name__ == "__main__":
    unittest.main()
