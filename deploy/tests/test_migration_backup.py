import copy
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import migration_backup
import test_rollout


class MigrationBackupTests(unittest.TestCase):
    def setUp(self):
        self.app = {
            "Config": {"Env": ["CPR_DATABASE_URL=postgres://example@db:5432/example"]},
            "NetworkSettings": {"Networks": {"example": {}}},
        }
        self.db = {
            "Config": {"Env": ["POSTGRES_USER=example", "POSTGRES_DB=example", "POSTGRES_PASSWORD=fixture"]},
            "NetworkSettings": {"Networks": {"example": {"Aliases": ["db"]}}},
            "State": {"Running": True},
        }

    def test_bind_database_only_on_shared_network_and_identity(self):
        self.assertEqual(migration_backup.database_container(self.app, {"db": self.db}), "db")
        for change in ("network", "database", "running", "duplicates"):
            db = copy.deepcopy(self.db)
            protected = {"db": db}
            if change == "network":
                db["NetworkSettings"]["Networks"] = {"other": {"Aliases": ["db"]}}
            elif change == "database":
                db["Config"]["Env"][1] = "POSTGRES_DB=other"
            elif change == "running":
                db["State"]["Running"] = False
            else:
                protected["other"] = copy.deepcopy(db)
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                migration_backup.database_container(self.app, protected)

    def test_schema_verification_checks_hashes_and_success(self):
        for output, accepted in [
            (b"1|abc|t\n", True), (b"1|abc|f\n", False),
            (b"1|changed|t\n", False), (b"1|abc|t\n2|def|t\n", False),
        ]:
            with patch.object(migration_backup, "database_command", return_value=
                              subprocess.CompletedProcess([], 0, stdout=output)):
                if accepted:
                    migration_backup.check_schema("db", {"1": "abc"})
                else:
                    with self.assertRaises(RuntimeError):
                        migration_backup.check_schema("db", {"1": "abc"})


class MigrationRolloutTests(unittest.TestCase):
    setUp = test_rollout.RolloutTests.setUp
    fake_up = test_rollout.RolloutTests.fake_up

    def test_migration_failure_never_rolls_back_old_image(self):
        self.worker.upgrade = {"before": {}, "after": {}}
        self.worker.migration_database = "example-db-1"
        calls = []
        def up(log_name):
            calls.append(log_name)
            self.fake_up(log_name)
            raise RuntimeError("simulated migration failure")
        with patch.object(migration_backup, "prepare"), \
                patch.object(migration_backup, "check_schema"), \
                patch.object(self.worker, "up", side_effect=up):
            self.assertEqual(self.worker.run(), 1)
        import json
        result = json.loads((self.worker.backup / "result.json").read_text())
        self.assertEqual(result["status"], "migration_recovery_required")
        self.assertEqual(calls, ["upgrade.log"])
        self.assertEqual(self.app["Image"], "sha256:new")

    def test_backup_failure_does_not_switch(self):
        self.worker.upgrade = {"before": {}, "after": {}}
        with patch.object(migration_backup, "prepare", side_effect=RuntimeError("backup failed")), \
                patch.object(self.worker, "up") as up, self.assertRaises(RuntimeError):
            self.worker.run()
        up.assert_not_called()
        self.assertEqual(self.app["Image"], "sha256:old")

    def test_backup_requires_complete_read_and_keeps_old_service_online(self):
        self.worker.prepare()
        data = self.directory / "runtime"
        data.mkdir()
        (data / "identity.json").write_text("{}")
        self.worker.old["Mounts"] = [
            {"Destination": "/app/.runtime/data", "Type": "bind", "Source": str(data)},
        ]
        self.worker.upgrade = {"before": {"1": "abc"}, "after": {"1": "abc"}}
        scripts = []
        def command(container, script, **kwargs):
            scripts.append(script)
            if script.startswith("pg_dump"):
                kwargs["stdout"].write(b"synthetic archive")
            return subprocess.CompletedProcess([], 0, stdout=b"1024")
        with patch.object(migration_backup, "database_container", return_value="db"), \
                patch.object(migration_backup, "check_schema"), \
                patch.object(migration_backup, "database_command", side_effect=command):
            migration_backup.prepare(self.worker)
        self.assertIn("pg_restore --file=/dev/null", scripts)
        self.assertTrue((self.worker.backup / "migration-backup.json").exists())
        self.assertEqual(self.app["Image"], "sha256:old")
        self.assertFalse(any("up" in args for args in self.commands))
