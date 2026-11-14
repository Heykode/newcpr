"""Copy the reviewed Excel-91 database into a genuinely migrated mainline database.

Never edit the source migration ledger. This is deliberately a version-specific
deployment operation, not an alternative application migration resolver.
"""

import hashlib
import json
import shlex
import subprocess
import tempfile
import uuid

import migration_backup


def identifier(value):
    return '"' + value.replace('"', '""') + '"'


def literal(value):
    return "'" + value.replace("'", "''") + "'"


class Database:
    def __init__(self, container, name):
        self.container = container
        self.name = name

    def at(self, name):
        return Database(self.container, name)

    def command(self, tool, *args):
        return ["docker", "exec", "-i", self.container, "sh", "-ec",
                'export PGPASSWORD="$POSTGRES_PASSWORD"; exec '
                + shlex.join([tool, "-h", "127.0.0.1", "-d", self.name])
                + ' -U "$POSTGRES_USER" ' + shlex.join(args)]

    def sql(self, statement):
        result = subprocess.run(
            self.command("psql", "-X", "-qAt", "-v", "ON_ERROR_STOP=1"),
            input="SET timezone='UTC'; SET statement_timeout='20min';\n" + statement,
            text=True, capture_output=True, timeout=1250,
        )
        if result.returncode:
            # SQL errors can quote a credential-bearing row; never echo stderr.
            raise RuntimeError("Database bridge SQL failed; source data is retained")
        return result.stdout.strip()

    def rows(self, query):
        return json.loads(self.sql(
            "SELECT coalesce(json_agg(q), '[]'::json) FROM (" + query + ") q;"))


def check_schema(database, expected):
    rows = database.rows("SELECT version, encode(checksum,'hex') AS checksum, success "
                         "FROM public._sqlx_migrations ORDER BY version")
    if (any(not row["success"] for row in rows)
            or {str(row["version"]): row["checksum"] for row in rows} != expected):
        raise RuntimeError("Database bridge migration baseline differs")


def initialize(database, migrations):
    """Execute every frozen SQL before recording it, only in a newly created database."""
    if database.rows("SELECT 1 FROM pg_tables WHERE schemaname='public'"):
        raise RuntimeError("Database bridge destination is not empty")
    statements = ["BEGIN; SET LOCAL cpr.fresh_install='true';", """
        CREATE TABLE public._sqlx_migrations (
            version bigint PRIMARY KEY, description text NOT NULL,
            installed_on timestamptz NOT NULL DEFAULT now(), success boolean NOT NULL,
            checksum bytea NOT NULL, execution_time bigint NOT NULL
        );"""]
    previous = 0
    for item in migrations:
        version = item["version"]
        checksum = hashlib.sha384(item["sql"].encode()).hexdigest()
        if (type(version) is not int or version <= previous
                or checksum != item["checksum"]):
            raise RuntimeError("Database bridge SQL identity differs")
        previous = version
        statements.extend([
            item["sql"],
            "INSERT INTO public._sqlx_migrations "
            "(version,description,success,checksum,execution_time) VALUES ("
            + str(version) + "," + literal(item["description"])
            + ",true,decode(" + literal(checksum) + ",'hex'),0);",
        ])
    statements.append("COMMIT;")
    database.sql("\n".join(statements))


def columns(database):
    rows = database.rows("""
        SELECT table_name, column_name FROM information_schema.columns
        WHERE table_schema='public' AND table_name <> '_sqlx_migrations'
        ORDER BY table_name, ordinal_position
    """)
    result = {}
    for row in rows:
        result.setdefault(row["table_name"], []).append(row["column_name"])
    return result


def fingerprints(database, tables):
    """Count rows and both halves of their MD5, without logging or collecting rows."""
    result = {}
    for table, fields in tables.items():
        query = "SELECT count(*) AS count, " + ",".join(
            "coalesce(sum(('x'||substr(h," + str(offset)
            + ",16))::bit(64)::bigint::numeric),0)::text AS h" + str(offset)
            for offset in (1, 17)
        ) + " FROM (SELECT md5(row_to_json(r)::text) AS h FROM (SELECT " \
            + ",".join(map(identifier, fields)) + " FROM public." + identifier(table) + ") r) s"
        result[table] = database.rows(query)[0]
    return result


def sequences(database):
    result = {}
    for row in database.rows("SELECT sequencename FROM pg_sequences WHERE schemaname='public' "
                             "ORDER BY sequencename"):
        name = row["sequencename"]
        result[name] = database.rows("SELECT last_value,is_called FROM public." + identifier(name))[0]
    return result


def copy_data(source, target):
    """Stream all business rows and sequences; no archive or source-ledger edits."""
    with tempfile.TemporaryFile() as dump_errors, tempfile.TemporaryFile() as load_errors:
        dump = subprocess.Popen(source.command(
            "pg_dump", "--data-only", "--no-owner", "--no-privileges", "--disable-triggers",
            "--schema=public", "--exclude-table=public._sqlx_migrations",
        ), stdout=subprocess.PIPE, stderr=dump_errors)
        load = None
        try:
            load = subprocess.Popen(target.command(
                "psql", "-X", "-q", "--single-transaction", "-v", "ON_ERROR_STOP=1",
            ), stdin=dump.stdout, stdout=subprocess.DEVNULL, stderr=load_errors)
            dump.stdout.close()
            load_code = load.wait(timeout=3600)
            dump_code = dump.wait(timeout=30)
            if dump_code or load_code:
                raise RuntimeError("Database bridge data copy failed; source data is retained")
        finally:
            for process in (load, dump):
                if process is not None and process.poll() is None:
                    process.kill()
                    process.wait()


def validate_constraints(database):
    constraints = database.rows("""
        SELECT c.conrelid::regclass::text AS relation, c.conname AS name,
               pg_get_constraintdef(c.oid) AS definition
        FROM pg_constraint c JOIN pg_namespace n ON n.oid=c.connamespace
        WHERE n.nspname='public' AND c.contype='f' ORDER BY c.oid
    """)
    statements = ["BEGIN;"]
    for item in constraints:
        # Definitions and relations come from this freshly initialized database.
        statements.extend([
            "ALTER TABLE " + item["relation"] + " DROP CONSTRAINT " + identifier(item["name"]) + ";",
            "ALTER TABLE " + item["relation"] + " ADD CONSTRAINT " + identifier(item["name"])
            + " " + item["definition"] + ";",
        ])
    statements.append("COMMIT;")
    database.sql("\n".join(statements))
    if database.rows("SELECT 1 FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid "
                     "JOIN pg_namespace n ON n.oid=c.relnamespace "
                     "WHERE n.nspname='public' AND t.tgenabled <> 'O'"):
        raise RuntimeError("Database bridge left an unexpected disabled trigger")


def transfer(source, target, upgrade):
    check_schema(source, upgrade["before"])
    initialize(target, upgrade["migration_sql"])
    check_schema(target, upgrade["after"])
    before = columns(source)
    after = columns(target)
    expected = {name: list(fields) for name, fields in before.items()}
    expected["runtime_settings"].append("quota_reset_window_weights_json")
    if expected != after:
        raise RuntimeError("Database bridge has an unreviewed column or table difference")
    source_rows = fingerprints(source, before)
    source_sequences = sequences(source)
    settings = source.rows("SELECT to_jsonb(s) AS value FROM runtime_settings s")[0]["value"]
    retention_days = source.rows("SELECT config #>> '{requests,retentionDays}' AS days "
                                 "FROM log_cleanup_control WHERE singleton")[0]["days"]
    target.sql("TRUNCATE " + ",".join("public." + identifier(name) for name in after) + ";")
    copy_data(source, target)
    if fingerprints(target, before) != source_rows or sequences(target) != source_sequences:
        raise RuntimeError("Database bridge data verification failed")
    validate_constraints(target)
    if fingerprints(source, before) != source_rows or sequences(source) != source_sequences:
        raise RuntimeError("Source database changed during the bridge")
    # Schema was genuinely initialized first. Apply the frozen retention operation
    # to the imported settings, rather than only to empty-database seed settings.
    retention = [item for item in upgrade["migration_sql"] if item["version"] == 91]
    if len(retention) != 1 or retention[0]["description"] != "usage retention days":
        raise RuntimeError("Database bridge retention operation differs")
    target.sql("BEGIN;\n" + retention[0]["sql"] + "\nCOMMIT;")
    expected_settings = {**settings, "quota_reset_window_weights_json":
                         {"fiveHour": 0, "weekly": 0, "monthly": 0}}
    if retention_days is not None and 1 <= int(retention_days) <= 30:
        expected_settings.update(usage_retention_days=int(retention_days),
                                 ops_event_retention_days=int(retention_days),
                                 config_revision=settings["config_revision"] + 1)
    if target.rows("SELECT to_jsonb(s) AS value FROM runtime_settings s")[0]["value"] != expected_settings:
        raise RuntimeError("Database bridge settings differ from the reviewed migration")
    check_schema(source, upgrade["before"])
    check_schema(target, upgrade["after"])
    return {"tables_verified": len(before), "rows_verified": sum(r["count"] for r in source_rows.values()),
            "sequences_verified": len(source_sequences)}


def prepare(worker):
    env = migration_backup.environment(worker.protected[worker.migration_database])
    name = env["POSTGRES_DB"]
    if name in {"postgres", "template0", "template1"}:
        raise RuntimeError("Database bridge requires a dedicated application database")
    source = Database(worker.migration_database, name)
    metadata = source.rows("SELECT to_jsonb(d) AS value FROM pg_database d "
                           "WHERE datname IN (current_database(),'template0')")
    settings = [item["value"] for item in metadata]
    source_meta = next(item for item in settings if item["datname"] == name)
    template_meta = next(item for item in settings if item["datname"] == "template0")
    for key in ("encoding", "datcollate", "datctype", "datlocprovider", "daticulocale", "datlocale",
                "dattablespace"):
        if source_meta.get(key) != template_meta.get(key):
            raise RuntimeError("Database bridge requires the reviewed default database locale")
    actual_version = source.sql("SELECT coalesce(pg_database_collation_actual_version("
                                + str(source_meta["oid"]) + "),'');")
    if (source_meta.get("datcollversion") or "") != actual_version:
        raise RuntimeError("Database bridge source collation requires a separate upgrade")
    if source.rows("SELECT 1 FROM pg_largeobject_metadata LIMIT 1"):
        raise RuntimeError("Database bridge does not support large objects")
    if source.rows("SELECT 1 FROM pg_namespace WHERE nspname NOT IN ('public','information_schema') "
                   "AND nspname NOT LIKE 'pg_%'"):
        raise RuntimeError("Database bridge found an unreviewed schema")
    if source.rows("SELECT 1 FROM pg_db_role_setting WHERE setdatabase=" + str(source_meta["oid"])):
        raise RuntimeError("Database bridge found custom database settings")
    if source_meta.get("datacl") is not None or source_meta.get("datconnlimit") != -1:
        raise RuntimeError("Database bridge found custom database access settings")
    if source.rows("SELECT 1 FROM pg_extension WHERE extname <> 'plpgsql'"):
        raise RuntimeError("Database bridge found an unreviewed extension")
    size = int(source.sql("SELECT pg_database_size(current_database());"))
    free = subprocess.run(["docker", "exec", worker.migration_database, "sh", "-ec",
                           'df -Pk "$PGDATA"'], capture_output=True, text=True, timeout=15)
    if free.returncode or int(free.stdout.splitlines()[-1].split()[3]) * 1024 < size * 2 + 1024**3:
        raise RuntimeError("Insufficient database volume space for a verified database copy")
    suffix = uuid.uuid4().hex[:16]
    worker.bridge = {"source": name, "target": "cpr_upgrade_" + suffix,
                     "retained": "cpr_previous_" + suffix, "owner": env["POSTGRES_USER"]}
    (worker.backup / "database-bridge.json").write_text(json.dumps(worker.bridge))


def switch(worker):
    result = worker.compose_run("stop", worker.profile["service"])
    (worker.backup / "drain.log").write_bytes(result.stdout)
    if result.returncode:
        raise RuntimeError("Application drain failed before database bridge")
    from rollout import inspect
    if inspect(worker.profile["container"])["State"]["Running"]:
        raise RuntimeError("Application is still running before database bridge")
    plan = worker.bridge
    source = Database(worker.migration_database, plan["source"])
    maintenance = source.at("postgres")
    active = maintenance.rows("SELECT 1 FROM pg_stat_activity WHERE datname=" + literal(source.name))
    if active:
        raise RuntimeError("Database bridge requires all application writers to be stopped")
    # Subsequent source connections default to read-only; retained history stays protected.
    maintenance.sql("ALTER DATABASE " + identifier(plan["source"])
                    + " SET default_transaction_read_only=on;")
    maintenance.sql("CREATE DATABASE " + identifier(plan["target"]) + " WITH TEMPLATE template0 OWNER "
                    + identifier(plan["owner"]) + ";")
    verification = transfer(source, source.at(plan["target"]), worker.upgrade)
    # Both renames commit together; app configuration and its database name stay unchanged.
    maintenance.sql("BEGIN; ALTER DATABASE " + identifier(plan["source"]) + " RENAME TO "
                    + identifier(plan["retained"]) + "; ALTER DATABASE " + identifier(plan["target"])
                    + " RENAME TO " + identifier(plan["source"]) + "; COMMIT;")
    check_schema(source, worker.upgrade["after"])
    (worker.backup / "database-bridge-result.json").write_text(json.dumps(verification))
    return verification
