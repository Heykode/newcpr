"""Focused bridge contracts, plus opt-in real PostgreSQL data-transfer coverage."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from urllib.parse import unquote, urlsplit
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import schema_bridge as bridge

ROOT = Path(__file__).resolve().parents[2]


def migrations():
    return [{"version": int(path.name.split("_")[0]),
             "description": path.stem.split("_", 1)[1].replace("_", " "),
             "sql": path.read_text(), "checksum": hashlib.sha384(path.read_bytes()).hexdigest()}
            for path in sorted((ROOT / "backend/migrations").glob("*.sql"))]


class BridgeTests(unittest.TestCase):
    def test_identifiers_and_literals_are_quoted(self):
        self.assertEqual(bridge.identifier('a"b'), '"a""b"')
        self.assertEqual(bridge.literal("a'b"), "'a''b'")

    def test_initializer_refuses_existing_tables_or_changed_sql(self):
        database = bridge.Database("example-db", "example")
        with patch.object(database, "rows", return_value=[{}]), \
                patch.object(database, "sql") as execute:
            with self.assertRaises(RuntimeError):
                bridge.initialize(database, migrations())
            execute.assert_not_called()
        altered = migrations()
        altered[0]["sql"] += "\n"
        with patch.object(database, "rows", return_value=[]), \
                patch.object(database, "sql") as execute:
            with self.assertRaises(RuntimeError):
                bridge.initialize(database, altered)
            execute.assert_not_called()

    def test_initializer_executes_sql_before_its_history_record(self):
        database = bridge.Database("example-db", "example")
        items = migrations()
        with patch.object(database, "rows", return_value=[]), patch.object(database, "sql") as execute:
            bridge.initialize(database, items)
        script = execute.call_args.args[0]
        self.assertTrue(script.startswith("BEGIN;"))
        self.assertTrue(script.endswith("COMMIT;"))
        for item in items:
            self.assertLess(script.index(item["sql"]), script.index(literal_checksum(item)))

    def test_bridge_failure_never_restarts_old_image(self):
        import test_rollout
        fixture = test_rollout.RolloutTests(methodName="test_failed_upgrade_rolls_back_pinned_old_image")
        fixture.setUp()
        try:
            fixture.worker.upgrade = {"mode": "excel-91-database-copy", "before": {}, "after": {}}
            fixture.worker.migration_database = "example-db-1"
            with patch("migration_backup.prepare"), patch("migration_backup.check_schema"), \
                    patch.object(bridge, "prepare"), \
                    patch.object(bridge, "switch", side_effect=RuntimeError("copy failed")), \
                    patch.object(fixture.worker, "up") as up:
                self.assertEqual(fixture.worker.run(), 1)
                up.assert_not_called()
            result = json.loads((fixture.worker.backup / "result.json").read_text())
            self.assertEqual(result["status"], "migration_recovery_required")
        finally:
            fixture.tearDown()
            fixture.doCleanups()


def literal_checksum(item):
    return "decode('" + item["checksum"] + "','hex')"


class LocalDatabase(bridge.Database):
    def __init__(self, endpoint, name):
        super().__init__("isolated", name)
        self.endpoint = endpoint

    def at(self, name):
        return LocalDatabase(self.endpoint, name)

    def command(self, tool, *args):
        return [tool, "-h", self.endpoint.hostname, "-p", str(self.endpoint.port or 5432),
                "-U", unquote(self.endpoint.username or ""), "-d", self.name, *args]


@unittest.skipUnless(os.environ.get("CPR_BRIDGE_TEST_DATABASE_URL")
                     or os.environ.get("CPR_BRIDGE_TEST_CONTAINER"), "isolated PostgreSQL opt-in required")
class PostgreSQLBridgeTests(unittest.TestCase):
    def test_real_legacy_data_to_mainline_and_atomic_names(self):
        container = os.environ.get("CPR_BRIDGE_TEST_CONTAINER")
        endpoint = urlsplit(os.environ.get("CPR_BRIDGE_TEST_DATABASE_URL", ""))
        if container:
            self.assertTrue(container.startswith("cpr-bridge-test-"))
            maintenance = bridge.Database(container, "postgres")
        else:
            self.assertIn(endpoint.hostname, ("localhost", "127.0.0.1"))
            maintenance = LocalDatabase(endpoint, endpoint.path.lstrip("/"))
        suffix = uuid.uuid4().hex[:12]
        names = ["cpr_bridge_test_" + suffix + value for value in ("_source", "_target", "_old", "_bad")]
        with patch.dict(os.environ, {"PGPASSWORD": unquote(endpoint.password or "")}):
            try:
                for name in names[:2]:
                    maintenance.sql("CREATE DATABASE " + bridge.identifier(name) + ";")
                source, target = [maintenance.at(name) for name in names[:2]]
                current = migrations()
                legacy = [dict(item) for item in current if item["version"] < 91]
                excel = next(dict(item) for item in current if item["version"] == 92)
                excel["version"] = 91
                legacy.append(excel)
                upgrade = {"migration_sql": current,
                           "before": {str(item["version"]): item["checksum"] for item in legacy},
                           "after": {str(item["version"]): item["checksum"] for item in current}}
                bridge.initialize(source, legacy)
                source.sql("""
                    INSERT INTO provider_accounts (id,provider_kind,name,upstream_user_id,
                        upstream_account_id,authentication_kind,provider_credentials_json,
                        has_refresh_token,credential_observed_at,created_at,updated_at)
                    VALUES ('account_bridge','openai','Example','user_bridge','tenant_bridge',
                            'oauth','{"fixture":"codex"}',false,now(),now(),now());
                    INSERT INTO openai_excel_credentials(account_id,upstream_user_id,credential_json,
                        credential_observed_at) VALUES ('account_bridge','user_bridge',
                        '{"fixture":"excel"}',now());
                    INSERT INTO account_cumulative_costs(provider_account_ref,currency,amount,request_count)
                        VALUES ('account_bridge','USD',123.45,12);
                    UPDATE runtime_settings SET max_concurrent_per_account=7 WHERE id=1;
                    UPDATE log_cleanup_control SET config=jsonb_set(config,'{requests,retentionDays}','7');
                """)
                original = source.rows("SELECT to_jsonb(s) AS value FROM runtime_settings s")[0]["value"]
                if container:
                    with tempfile.TemporaryDirectory() as directory:
                        worker = SimpleNamespace(migration_database=container, backup=Path(directory),
                            protected={container: {"Config": {"Env": ["POSTGRES_DB=" + names[0],
                                                                        "POSTGRES_USER=postgres"]}}})
                        bridge.prepare(worker)
                        self.assertEqual(worker.bridge["source"], names[0])
                        self.assertEqual(maintenance.sql("SELECT count(*) FROM pg_database WHERE datname="
                                                       + bridge.literal(worker.bridge["target"])), "0")
                maintenance.sql("ALTER DATABASE " + bridge.identifier(names[0])
                                + " SET default_transaction_read_only=on;")
                with self.assertRaises(RuntimeError):
                    source.sql("UPDATE runtime_settings SET max_concurrent_per_account=8")
                maintenance.sql("CREATE DATABASE " + bridge.identifier(names[3]) + ";")
                copy = bridge.copy_data

                def corrupt_copy(old, new):
                    copy(old, new)
                    new.sql("UPDATE account_cumulative_costs SET amount=0")

                with patch.object(bridge, "copy_data", side_effect=corrupt_copy):
                    with self.assertRaisesRegex(RuntimeError, "data verification failed"):
                        bridge.transfer(source, maintenance.at(names[3]), upgrade)
                bridge.check_schema(source, upgrade["before"])
                stats = bridge.transfer(source, target, upgrade)
                self.assertGreater(stats["tables_verified"], 50)
                bridge.check_schema(source, upgrade["before"])
                bridge.check_schema(target, upgrade["after"])
                settings = target.rows("SELECT to_jsonb(s) AS value FROM runtime_settings s")[0]["value"]
                self.assertEqual(settings["max_concurrent_per_account"], 7)
                self.assertEqual(settings["usage_retention_days"], 7)
                self.assertEqual(settings["config_revision"], original["config_revision"] + 1)
                self.assertEqual(settings["quota_reset_window_weights_json"],
                                 {"fiveHour": 0, "weekly": 0, "monthly": 0})
                self.assertEqual(target.sql("SELECT amount FROM account_cumulative_costs"), "123.4500000000")
                self.assertEqual(target.sql("SELECT credential_json->>'fixture' FROM openai_excel_credentials"),
                                 "excel")
                with self.assertRaises(RuntimeError):
                    bridge.initialize(target, current)
                # A name collision must roll back the first rename too.
                with self.assertRaises(RuntimeError):
                    maintenance.sql("BEGIN; ALTER DATABASE " + bridge.identifier(names[0])
                                    + " RENAME TO " + bridge.identifier(names[2]) + "; ALTER DATABASE "
                                    + bridge.identifier(names[1]) + " RENAME TO "
                                    + bridge.identifier(maintenance.name) + "; COMMIT;")
                bridge.check_schema(source, upgrade["before"])
                maintenance.sql("BEGIN; ALTER DATABASE " + bridge.identifier(names[0])
                                + " RENAME TO " + bridge.identifier(names[2]) + "; ALTER DATABASE "
                                + bridge.identifier(names[1]) + " RENAME TO "
                                + bridge.identifier(names[0]) + "; COMMIT;")
                bridge.check_schema(maintenance.at(names[0]), upgrade["after"])
                bridge.check_schema(maintenance.at(names[2]), upgrade["before"])
            finally:
                for name in names:
                    maintenance.sql("DROP DATABASE IF EXISTS " + bridge.identifier(name) + " WITH (FORCE);")


if __name__ == "__main__":
    unittest.main()
