import contextlib
import copy
import fcntl
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import rollout


class RolloutTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.compose = self.directory / "compose.yaml"
        self.original = {
            "name": "example", "services": {
                "app": {"image": "example:old", "environment": {"MODE": "test"},
                        "ports": ["127.0.0.1:8080:8080"]},
                "db": {"image": "example:db", "volumes": ["data:/data"]},
            }, "volumes": {"data": {}},
        }
        self.compose.write_text(yaml.safe_dump(self.original))
        self.compose.chmod(0o640)
        self.config = self.directory / "config.yaml"
        self.config.write_text("mode: test\n")
        self.archive = self.directory / "image.tar.gz"
        self.archive.write_bytes(b"synthetic archive")
        self.request = {
            "profile": {
                "compose_file": str(self.compose), "container": "example-app-1", "service": "app",
                "protected_containers": ["example-db-1"], "config_files": [str(self.config)],
                "health_url": "http://127.0.0.1:8080/healthz",
            },
            "expected_old_image": "sha256:old",
            "selected": {"target_commit": "a" * 40, "proof": {
                "archive_sha256": rollout.digest(self.archive), "image_tag": "example:new",
                "source_commit": "b" * 40, "version": "1.0.1",
            }},
        }
        self.app = {
            "Id": "old-container", "Image": "sha256:old", "RestartCount": 0,
            "Config": {"StopTimeout": None, "Labels": {
                "com.docker.compose.service": "app", "com.docker.compose.project": "example",
                "com.docker.compose.project.config_files": str(self.compose),
            }},
            "State": {"Running": True, "StartedAt": "before"},
        }
        self.db = {"Id": "db-container", "State": {"StartedAt": "before"}}
        self.image = {
            "Id": "sha256:new", "Architecture": "amd64", "Os": "linux",
            "Config": {"Labels": {
                "org.opencontainers.image.revision": "b" * 40,
                "org.opencontainers.image.version": "1.0.1",
            }},
        }
        self.commands = []

        def inspect(name):
            return copy.deepcopy({
                "example-app-1": self.app, "example-db-1": self.db, "example:new": self.image,
            }[name])

        def run(args, **kwargs):
            self.commands.append(args)
            return subprocess.CompletedProcess(args, 0, stdout=b"", stderr=b"")

        for item in [
            patch.object(rollout, "inspect", side_effect=inspect),
            patch.object(rollout, "healthy", return_value=True),
            patch.object(rollout.subprocess, "run", side_effect=run),
            patch.object(rollout.time, "sleep"),
        ]:
            item.start()
            self.addCleanup(item.stop)
        self.output = contextlib.redirect_stdout(io.StringIO())
        self.output.__enter__()
        self.addCleanup(self.output.__exit__, None, None, None)
        self.worker = rollout.Rollout(self.request, self.archive)

    def fake_up(self, log_name):
        image = yaml.safe_load(self.compose.read_text())["services"]["app"]["image"]
        self.app["Image"] = image
        self.app["Id"] = image + "-container"

    def test_prepare_does_not_change_live_configuration(self):
        before = self.compose.read_bytes()
        self.worker.prepare()
        self.assertEqual(self.compose.read_bytes(), before)
        candidate = yaml.safe_load(self.worker.pending.read_text())
        expected = rollout.changed_image(self.original, "app", "sha256:new")
        self.assertEqual(candidate, expected)
        self.assertEqual(self.worker.pending.stat().st_mode & 0o777, 0o640)
        self.assertFalse(any("up" in args for args in self.commands))
        self.assertEqual(yaml.safe_load((self.worker.backup / "rollback.yaml").read_text())
                         ["services"]["app"]["image"], "sha256:old")

    def test_wrong_archive_stops_before_docker_load(self):
        self.archive.write_bytes(b"corrupt")
        with self.assertRaisesRegex(RuntimeError, "checksum"):
            self.worker.prepare()
        self.assertEqual(self.commands, [])

    def test_invalid_candidate_leaves_no_pending_file(self):
        with patch.object(self.worker, "compose_run", return_value=subprocess.CompletedProcess([], 1)), \
                self.assertRaises(RuntimeError):
            self.worker.run()
        self.assertFalse(self.worker.pending.exists())
        self.assertEqual(yaml.safe_load(self.compose.read_text()), self.original)

    def test_wrong_imported_revision_keeps_old_service(self):
        self.image["Config"]["Labels"]["org.opencontainers.image.revision"] = "c" * 40
        with self.assertRaisesRegex(RuntimeError, "identity"):
            self.worker.prepare()
        self.assertEqual(yaml.safe_load(self.compose.read_text()), self.original)
        self.assertFalse(any("up" in args for args in self.commands))

    def test_wrong_project_file_is_rejected(self):
        self.app["Config"]["Labels"]["com.docker.compose.project.config_files"] += ",override.yaml"
        with self.assertRaisesRegex(RuntimeError, "single-file"):
            self.worker.prepare()

    def test_success(self):
        with patch.object(self.worker, "up", side_effect=self.fake_up):
            self.assertEqual(self.worker.run(), 0)
        result = json.loads((self.worker.backup / "result.json").read_text())
        self.assertEqual(result["status"], "deployed")
        self.assertEqual(self.app["Image"], "sha256:new")
        self.assertTrue((self.directory / ".cpr-release.json").exists())
        self.assertEqual(self.db["Id"], "db-container")
        self.assertEqual(self.config.read_text(), "mode: test\n")

    def test_failed_upgrade_rolls_back_pinned_old_image(self):
        def up(log_name):
            self.fake_up(log_name)
            if log_name == "upgrade.log":
                raise RuntimeError("injected failure")
        with patch.object(self.worker, "up", side_effect=up):
            self.assertEqual(self.worker.run(), 1)
        result = json.loads((self.worker.backup / "result.json").read_text())
        self.assertEqual(result["status"], "rolled_back")
        self.assertEqual(self.app["Image"], "sha256:old")
        self.assertFalse((self.directory / ".cpr-release.json").exists())
        restored = yaml.safe_load(self.compose.read_text())
        self.assertEqual(restored, rollout.changed_image(self.original, "app", "sha256:old"))

    def test_rollback_failure_is_explicit(self):
        with patch.object(self.worker, "up", side_effect=RuntimeError("injected failure")):
            self.assertEqual(self.worker.run(), 1)
        self.assertEqual(json.loads((self.worker.backup / "result.json").read_text())
                         ["status"], "rollback_failed")

    def test_protected_container_change_triggers_rollback(self):
        def up(log_name):
            self.fake_up(log_name)
            if log_name == "upgrade.log":
                self.db["State"]["StartedAt"] = "changed"
        with patch.object(self.worker, "up", side_effect=up):
            self.assertEqual(self.worker.run(), 1)
        self.assertEqual(self.app["Image"], "sha256:old")

    def test_concurrent_config_change_is_not_overwritten(self):
        self.worker.prepare()
        self.compose.write_text("changed: externally\n")
        with self.assertRaisesRegex(RuntimeError, "configuration changed"), \
                patch.object(self.worker, "up") as up:
            self.worker.switch()
        up.assert_not_called()
        self.assertEqual(self.compose.read_text(), "changed: externally\n")

    def test_concurrent_account_config_change_is_not_overwritten(self):
        self.worker.prepare()
        self.config.write_text("mode: changed\n")
        with self.assertRaisesRegex(RuntimeError, "configuration changed"):
            self.worker.switch()
        self.assertEqual(self.config.read_text(), "mode: changed\n")

    def test_concurrent_container_change_is_rejected(self):
        self.worker.prepare()
        self.app["Id"] = "other-container"
        with self.assertRaisesRegex(RuntimeError, "Another deployment"):
            self.worker.switch()

    def test_shared_lock_excludes_another_deployer(self):
        with self.compose.with_name(".cpr-deployment.lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaises(BlockingIOError), patch.object(self.worker, "prepare") as prepare:
                self.worker.run()
            prepare.assert_not_called()

    def test_compose_command_only_recreates_application(self):
        self.worker.prepare()
        self.worker.up("command.log")
        args = self.commands[-1]
        self.assertEqual(args[-7:], ["up", "-d", "--no-deps", "--no-build", "--pull", "never", "app"])
        self.assertIn("--project-name", args)
        self.assertNotIn("down", args)


class HealthGapTests(unittest.TestCase):
    def test_continuously_healthy(self):
        self.assertEqual(rollout.health_gap([(0, True), (1, True)]), 0)

    def test_gap_bracketed_by_healthy_observations(self):
        self.assertEqual(rollout.health_gap([(0, True), (0.2, False), (0.4, False), (0.6, True)]), 0.6)

    def test_unrecovered_gap(self):
        self.assertEqual(rollout.health_gap([(0, True), (0.2, False), (0.4, False)]), 0.4)


if __name__ == "__main__":
    unittest.main()
