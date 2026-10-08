import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class CachedBackendBuildTests(unittest.TestCase):
    def fixture(self, temporary):
        root = Path(temporary)
        for directory in ("release", "backend/apps", "backend/crates", "backend/migrations", "bin"):
            (root / directory).mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / "release/build_ci_backend.sh", root / "release/build_ci_backend.sh")
        for path in ("backend/Cargo.toml", "backend/Cargo.lock", "backend/apps/source.rs",
                     "backend/crates/source.rs", "backend/migrations/0001_fixture.sql", "release/version.yaml"):
            (root / path).write_text("fixture\n")
        (root / "backend/rust-toolchain.toml").write_text("must not select the checkout toolchain\n")
        docker = root / "bin/docker"
        docker.write_text('''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys
args = sys.argv[1:]
volumes = [args[i + 1] for i, value in enumerate(args) if value == "--volume"]
source = Path(next(value[:-8] for value in volumes if value.endswith(":/app:ro")))
cache = Path(next(value[:-7] for value in volumes if value.endswith(":/cargo")))
record = {"args": args, "files": sorted(str(p.relative_to(source)) for p in source.rglob("*") if p.is_file()),
          "metadata": {name: os.environ[name] for name in ("CPR_VERSION", "CPR_GIT_SHA", "CPR_BUILD_TIME")}}
Path(os.environ["RECORD"]).write_text(json.dumps(record))
if os.environ.get("FAIL_BUILD"):
    sys.exit(23)
(cache / "target/release").mkdir(parents=True, exist_ok=True)
(cache / "target/release/codex-proxy-rs").write_text(os.environ["CPR_GIT_SHA"])
''')
        docker.chmod(0o755)
        env = {**os.environ, "PATH": str(root / "bin") + os.pathsep + os.environ["PATH"],
               "CPR_CARGO_CACHE": str(root / "cache"), "CPR_BACKEND_IMAGE": "fixture-toolchain",
               "CPR_VERSION": "1.2.3", "CPR_GIT_SHA": "a" * 40,
               "CPR_BUILD_TIME": "2026-01-01T00:00:00Z", "RUNNER_TEMP": str(root),
               "RECORD": str(root / "record.json")}
        return root, env

    def test_real_script_stages_exact_inputs_and_rebuilds_current_metadata(self):
        with tempfile.TemporaryDirectory() as temporary:
            root, env = self.fixture(temporary)
            for revision in ("a" * 40, "b" * 40):
                env["CPR_GIT_SHA"] = revision
                subprocess.run(["bash", str(root / "release/build_ci_backend.sh")], env=env, check=True)
                record = json.loads((root / "record.json").read_text())
                self.assertEqual(record["files"], [
                    "backend/Cargo.lock", "backend/Cargo.toml", "backend/apps/source.rs",
                    "backend/crates/source.rs", "backend/migrations/0001_fixture.sql", "release/version.yaml",
                ])
                self.assertEqual(record["metadata"]["CPR_GIT_SHA"], revision)
                self.assertEqual(record["args"][-7:],
                                 ["fixture-toolchain", "cargo", "build", "--release", "--locked", "--bin", "codex-proxy-rs"])
                self.assertIn("CARGO_TARGET_DIR=/cargo/target", record["args"])
                self.assertEqual((root / "dist/docker/linux-amd64/codex-proxy-rs").read_text(), revision)
                self.assertEqual(list(root.glob("cpr-ci-backend.*")), [])

    def test_failed_build_never_copies_an_old_cached_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            root, env = self.fixture(temporary)
            binary = root / "cache/target/release/codex-proxy-rs"
            binary.parent.mkdir(parents=True)
            binary.write_text("old fixture")
            result = subprocess.run(["bash", str(root / "release/build_ci_backend.sh")], env={**env, "FAIL_BUILD": "1"})
            self.assertEqual(result.returncode, 23)
            self.assertFalse((root / "dist/docker/linux-amd64/codex-proxy-rs").exists())
            self.assertEqual(list(root.glob("cpr-ci-backend.*")), [])

    def test_invalid_metadata_stops_before_docker(self):
        with tempfile.TemporaryDirectory() as temporary:
            root, env = self.fixture(temporary)
            result = subprocess.run(["bash", str(root / "release/build_ci_backend.sh")],
                                    env={**env, "CPR_GIT_SHA": "branch-name"})
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((root / "record.json").exists())


if __name__ == "__main__":
    unittest.main()
