"""Backup and verify a reviewed migration upgrade; never restore a live database."""

import json
from pathlib import Path
import shutil
import subprocess
import tarfile
from urllib.parse import unquote, urlsplit


def environment(container):
    return dict(item.split("=", 1) for item in container["Config"].get("Env", []) if "=" in item)


def database_container(app, protected):
    endpoint = urlsplit(environment(app).get("CPR_DATABASE_URL", ""))
    if (endpoint.scheme not in {"postgres", "postgresql"} or endpoint.query
            or endpoint.fragment or endpoint.port not in {None, 5432}):
        raise RuntimeError("Migration backup requires the configured Compose PostgreSQL database")
    matches = []
    for name, container in protected.items():
        env = environment(container)
        if not all(env.get(key) for key in ("POSTGRES_USER", "POSTGRES_DB", "POSTGRES_PASSWORD")):
            continue
        common = set(app["NetworkSettings"]["Networks"]) & set(container["NetworkSettings"]["Networks"])
        aliases = {
            alias for network in common
            for alias in (container["NetworkSettings"]["Networks"][network].get("Aliases") or [])
        }
        if (endpoint.hostname in aliases and unquote(endpoint.username or "") == env["POSTGRES_USER"]
                and unquote(endpoint.path.lstrip("/")) == env["POSTGRES_DB"]
                and container["State"]["Running"]):
            matches.append(name)
    if len(matches) != 1:
        raise RuntimeError("Cannot uniquely bind backup to the application database")
    return matches[0]


def database_command(container, script, **kwargs):
    return subprocess.run(
        ["docker", "exec", "-i", container, "sh", "-ec",
         'export PGPASSWORD="$POSTGRES_PASSWORD"; ' + script],
        stderr=subprocess.PIPE, timeout=600, **kwargs,
    )


def check_schema(container, expected):
    result = database_command(
        container,
        """psql -X -h 127.0.0.1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -At """
        """-v ON_ERROR_STOP=1 -c "select version, encode(checksum, 'hex'), success """
        """from public._sqlx_migrations order by version" """,
        stdout=subprocess.PIPE,
    )
    if result.returncode:
        raise RuntimeError("Cannot verify application migration history")
    actual = {}
    for line in result.stdout.decode().splitlines():
        version, checksum, success = line.split("|")
        if success != "t":
            raise RuntimeError("Application database has an unsuccessful migration")
        actual[version] = checksum
    if actual != expected:
        raise RuntimeError("Application database differs from the reviewed migration baseline")


def prepare(worker):
    container = database_container(worker.old, worker.protected)
    worker.migration_database = container
    check_schema(container, worker.upgrade["before"])
    mounts = [mount for mount in worker.old.get("Mounts", [])
              if mount["Destination"] == "/app/.runtime/data" and mount["Type"] == "bind"]
    if len(mounts) != 1:
        raise RuntimeError("Migration backup requires a single application data bind mount")
    data = Path(mounts[0]["Source"])
    if not data.is_dir() or data.is_symlink():
        raise RuntimeError("Application data mount is not a regular directory")
    size = database_command(
        container,
        'psql -X -h 127.0.0.1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -At '
        '-v ON_ERROR_STOP=1 -c "select pg_database_size(current_database())"',
        stdout=subprocess.PIPE,
    )
    if size.returncode:
        raise RuntimeError("Cannot estimate database backup space")
    runtime_size = sum(path.stat().st_size for path in data.rglob("*")
                       if not path.is_symlink() and path.is_file())
    if shutil.disk_usage(worker.backup).free < int(size.stdout) + runtime_size + 1024**3:
        raise RuntimeError("Insufficient free space for migration backups and safety reserve")
    archive = worker.backup / "database.dump"
    with archive.open("xb") as output:
        result = database_command(
            container,
            'pg_dump -h 127.0.0.1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" '
            '--format=custom --no-owner --no-privileges',
            stdout=output,
        )
    archive.chmod(0o600)
    if result.returncode or archive.stat().st_size == 0:
        raise RuntimeError("Online database backup failed")
    # Reading only the table of contents would not detect truncated data blocks.
    with archive.open("rb") as source:
        result = database_command(
            container, "pg_restore --file=/dev/null", stdin=source, stdout=subprocess.DEVNULL,
        )
    if result.returncode:
        raise RuntimeError("Database backup could not be read in full")
    runtime = worker.backup / "runtime-data.tar"
    # This is an online copy, not an atomic application/database recovery point.
    with tarfile.open(runtime, "w") as output:
        output.add(data, arcname="runtime-data", recursive=True)
    runtime.chmod(0o600)
    with tarfile.open(runtime, "r") as source:
        for member in source:
            if member.isfile():
                with source.extractfile(member) as stream:
                    while stream.read(1024 * 1024):
                        pass
    from rollout import digest
    (worker.backup / "migration-backup.json").write_text(json.dumps({
        "plan": worker.upgrade,
        "database_sha256": digest(archive),
        "runtime_sha256": digest(runtime),
        "online": True,
        "recovery": "manual; online backups predate cutover and may omit later writes",
    }, indent=2))
