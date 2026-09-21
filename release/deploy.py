"""Deploy a verified CI image; default mode is a read-only deployment plan."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
from urllib.parse import urlsplit

import verified_image as images

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "deploy"))
import egress_check

DEPLOY_FILES = [
    "release/deploy.py", "release/verified_image.py", "deploy/rollout.py",
    "deploy/migration_backup.py", "deploy/egress_check.py",
]


def ssh(host, *args):
    return images.command("ssh", "-o", "BatchMode=yes", host, shlex.join(args))


def validate_profile(profile):
    if not isinstance(profile, dict):
        raise images.Unavailable("Deployment profile must be an object")
    for field in ("repository", "ssh_host", "compose_file", "container", "service", "health_url"):
        if not isinstance(profile.get(field), str):
            raise images.Unavailable("Missing or invalid deployment profile field")
    if not images.REPOSITORY.fullmatch(profile["repository"]):
        raise images.Unavailable("Invalid repository")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", profile.get("ssh_host", "")):
        raise images.Unavailable("Use a configured SSH alias, not inline SSH options")
    for field in ("compose_file",):
        if not Path(profile.get(field, "")).is_absolute():
            raise images.Unavailable("The server Compose path must be absolute")
    for field in ("container", "service"):
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", profile.get(field, "")):
            raise images.Unavailable("Invalid container or service")
    endpoint = urlsplit(profile["health_url"])
    if endpoint.scheme not in {"http", "https"} or not endpoint.hostname or endpoint.username:
        raise images.Unavailable("Use an unauthenticated HTTP(S) health endpoint")
    if type(profile.get("health_status", 204)) is not int or not 200 <= profile.get("health_status", 204) < 300:
        raise images.Unavailable("Health status must be a successful HTTP status")
    protected = profile.get("protected_containers")
    if not isinstance(protected, list) or not protected:
        raise images.Unavailable("Health endpoint and protected containers are required")
    if any(not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", name)
           for name in protected):
        raise images.Unavailable("Invalid protected container")
    configs = profile.get("config_files", [])
    if not isinstance(configs, list) or any(
            not isinstance(path, str) or not Path(path).is_absolute() for path in configs):
        raise images.Unavailable("Configuration paths must be absolute")
    if profile["container"] in profile["protected_containers"]:
        raise images.Unavailable("The application cannot also be a protected container")
    try:
        egress_check.validate_checks(profile.get("egress_checks", []))
    except (TypeError, ValueError):
        raise images.Unavailable("Invalid deployment egress checks") from None
    return profile


def current_image(profile):
    host = profile["ssh_host"]
    container = json.loads(ssh(host, "docker", "inspect", profile["container"]))[0]
    image = json.loads(ssh(host, "docker", "image", "inspect", container["Image"]))[0]
    if not container["State"]["Running"]:
        raise images.Unavailable("The current service is not running")
    if image["Architecture"] != "amd64" or image["Os"] != "linux":
        raise images.Unavailable("The fast deployment path supports linux/amd64 only")
    return container["Image"], image["Config"].get("Labels", {}) or {}


def validate_upgrade(old_labels, proof):
    source = old_labels.get("org.opencontainers.image.revision", "")
    if not images.SHA.fullmatch(source):
        raise images.Unavailable("Current image lacks a source revision; establish a reviewed baseline first")
    migrations = images.git("rev-parse", source + ":backend/migrations")
    if migrations != proof["migrations_tree"]:
        raise images.Unavailable("Database migrations changed; use a separately reviewed migration deployment")
    old_version = old_labels.get("org.opencontainers.image.version", "")
    if not old_version or old_version.split(".")[0] != proof["version"].split(".")[0]:
        raise images.Unavailable("Major-version changes are not eligible for automatic rollback")


def reviewed_upgrade(name, old_labels, proof, commit):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", name):
        raise images.Unavailable("Invalid reviewed upgrade name")
    path = "deploy/upgrades/" + name + ".json"
    plan_text = images.git("show", commit + ":" + path)
    if plan_text != images.git("show", "origin/main:" + path):
        raise images.Unavailable("Upgrade plan differs from main")
    plan = json.loads(plan_text)
    source = old_labels.get("org.opencontainers.image.revision", "")
    if not images.SHA.fullmatch(source):
        raise images.Unavailable("Current image lacks a reviewed source revision")
    if (plan["from_version"] != old_labels.get("org.opencontainers.image.version")
            or plan["to_version"] != proof["version"]
            or plan["from_version"].split(".")[0] != plan["to_version"].split(".")[0]
            or plan["from_migrations_tree"] != images.git("rev-parse", source + ":backend/migrations")
            or plan["to_migrations_tree"] != proof["migrations_tree"]
            or plan["recovery"] != "manual"):
        raise images.Unavailable("Deployment does not match the reviewed migration plan")
    changes = images.git("diff", "--name-status", source, commit, "--", "backend/migrations").splitlines()
    expected = {"A\tbackend/migrations/" + item for item in plan["added"]}
    expected.add("M\tbackend/migrations/.frozen-sha256")
    if set(changes) != expected:
        raise images.Unavailable("Migration changes differ from the reviewed additions")
    # Preserve exact bytes: git() strips trailing whitespace, SQLx checksums do not.
    def checksums(ref):
        paths = images.git("ls-tree", "-r", "--name-only", ref, "backend/migrations").splitlines()
        return {
            str(int(Path(path).name.split("_", 1)[0])): hashlib.sha384(
                subprocess.check_output(["git", "show", ref + ":" + path])
            ).hexdigest()
            for path in paths if path.endswith(".sql")
        }
    return {**plan, "before": checksums(source), "after": checksums(commit)}


def require_target_ci(repository, commit):
    workflow = images.api(f"repos/{repository}/actions/workflows/ci.yml")["id"]
    runs = images.api(
        f"repos/{repository}/actions/workflows/{workflow}/runs?head_sha={commit}&per_page=20"
    )["workflow_runs"]
    runs = [run for run in runs if run["event"] in {"push", "workflow_dispatch"}]
    if not runs or not images.trusted_run(max(runs, key=lambda run: run["id"]), repository, commit, workflow):
        raise images.Unavailable("Latest main CI must succeed before deployment")


def run(profile, commit, apply, migration_plan=None, *, prepare=False, staged=None):
    if (apply and prepare) or (staged is not None and not apply):
        raise images.Unavailable("Use --prepare or --apply; --staged requires --apply")
    if staged is not None and not re.fullmatch(r"/var/tmp/cpr-deploy\.[A-Za-z0-9]+", staged):
        raise images.Unavailable("Invalid prepared staging directory")
    os.chdir(ROOT)
    images.git("fetch", "origin", "main")
    # Do not silently execute a stale or locally edited deployment implementation.
    for path in DEPLOY_FILES:
        if images.git("hash-object", path) != images.git("rev-parse", "origin/main:" + path):
            raise images.Unavailable("Update the deployment scripts from main before deploying")
    subprocess.run(["git", "merge-base", "--is-ancestor", commit, "origin/main"], check=True)
    repository = images.command("gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner")
    if repository != profile["repository"]:
        raise images.Unavailable("Deployment profile belongs to a different repository")
    require_target_ci(repository, commit)
    images.require_security(repository, commit)
    selected = images.resolve(repository, commit)
    old_id, old_labels = current_image(profile)
    upgrade = None
    if migration_plan:
        upgrade = reviewed_upgrade(migration_plan, old_labels, selected["proof"], commit)
    else:
        validate_upgrade(old_labels, selected["proof"])
    print(json.dumps({
        "mode": "apply" if apply else "prepare" if prepare else "plan",
        "target_commit": commit, "tested_commit": selected["proof"]["source_commit"],
        "ci_run": selected["proof"]["run_id"], "platform": selected["proof"]["platform"],
        "build_required": False, "migrations_changed": upgrade is not None,
        "automatic_image_rollback": upgrade is None,
    }), flush=True)
    if not apply and not prepare:
        return
    host = profile["ssh_host"]
    with tempfile.TemporaryDirectory(prefix="cpr-deploy-") as temporary:
        directory = Path(temporary)
        archive = directory / "image.tar.gz"
        request = {"profile": profile, "selected": selected, "expected_old_image": old_id}
        if upgrade:
            request["migration_upgrade"] = upgrade
        if staged is not None:
            previous = json.loads(ssh(host, "cat", staged + "/request.json"))
            if previous != request:
                raise images.Unavailable("Prepared request changed; run a new preparation before applying")
            remote = staged
            transfer = []
            print("Reusing the uploaded image; rechecking and refreshing online backups before switching.",
                  flush=True)
        else:
            print("Downloading and verifying the CI image; the old service stays online.", flush=True)
            images.artifact_file(repository, selected["artifact"], "image.tar.gz", archive)
            if images.sha256(archive) != selected["proof"]["archive_sha256"]:
                raise images.Unavailable("Image archive checksum mismatch")
            remote = ssh(host, "mktemp", "-d", "/var/tmp/cpr-deploy.XXXXXXXX")
            if not re.fullmatch(r"/var/tmp/cpr-deploy\.[A-Za-z0-9]+", remote):
                raise images.Unavailable("Unexpected remote staging directory")
            transfer = [str(archive)]
        request_path = directory / "request.json"
        request_path.write_text(json.dumps(request))
        request_path.chmod(0o600)
        subprocess.run([
            "scp", *transfer, str(request_path), str(ROOT / "deploy/rollout.py"),
            str(ROOT / "deploy/migration_backup.py"), str(ROOT / "deploy/egress_check.py"),
            host + ":" + remote + "/",
        ], check=True)
        if prepare:
            print("Image staged. Preparing online only; application switching is disabled.", flush=True)
        else:
            print("Image staged. Starting locked deployment"
                  + (" with reviewed migrations and manual recovery." if upgrade
                     else " with image rollback enabled."), flush=True)
        # Keep SSH alive during graceful drain; never kill it to force a short outage.
        result = subprocess.run([
            "ssh", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=15", host,
            shlex.join(["python3", remote + "/rollout.py", "--request",
                        remote + "/request.json", "--archive", remote + "/image.tar.gz",
                        "--prepare" if prepare else "--apply"]),
        ])
        if result.returncode:
            raise images.Unavailable("Preparation or deployment failed; inspect the private result before retrying")
        if prepare:
            print(json.dumps({"status": "prepared_not_deployed", "staged": remote,
                              "target_commit": commit, "version": selected["proof"]["version"]}),
                  flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", required=True, type=Path)
    parser.add_argument("--commit", required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--apply", action="store_true")
    mode.add_argument("--prepare", action="store_true", help="Stage image and online backups, never switch")
    parser.add_argument("--staged", help="Reuse a prepared server directory with --apply")
    parser.add_argument("--migration-plan", help="Reviewed upgrade name, for example v3.13.0")
    args = parser.parse_args()
    if not images.SHA.fullmatch(args.commit):
        parser.error("--commit requires the full commit SHA")
    profile_path = args.profile.resolve()
    if profile_path == ROOT or ROOT in profile_path.parents:
        parser.error("Keep real deployment profiles outside the repository")
    if profile_path.stat().st_mode & 0o077:
        parser.error("Deployment profile must be private (chmod 600)")
    try:
        profile = validate_profile(json.loads(profile_path.read_text()))
        run(profile, args.commit, args.apply, args.migration_plan,
            prepare=args.prepare, staged=args.staged)
    except (images.Unavailable, subprocess.SubprocessError, OSError, ValueError, KeyError) as error:
        # Never echo subprocess commands that may contain private deployment paths.
        message = str(error) if isinstance(error, images.Unavailable) else type(error).__name__
        parser.exit(1, "Deployment stopped: " + message + "\n")


if __name__ == "__main__":
    main()
