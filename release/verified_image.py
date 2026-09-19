"""Resolve immutable CI images. GitHub run provenance is checked before download."""

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import zipfile

MAX_AGE = timedelta(days=7)
SHA = re.compile(r"[0-9a-f]{40}")
DIGEST = re.compile(r"sha256:[0-9a-f]{64}")
REPOSITORY = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*")
SECURITY_JOBS = {"Privacy Guard", "backend-security", "frontend-security"}


class Unavailable(RuntimeError):
    pass


def command(*args):
    return subprocess.check_output(args, text=True, timeout=120).strip()


def git(*args):
    return command("git", *args)


def api(path):
    return json.loads(command("gh", "api", path))


def sha256(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def timestamp(value):
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def fresh(value, now=None):
    now = now or datetime.now(timezone.utc)
    try:
        age = now - timestamp(value)
    except (AttributeError, TypeError, ValueError):
        return False
    return timedelta(0) <= age <= MAX_AGE


def candidates(commit):
    """Only the exact commit or an identical-tree merge parent may be reused."""
    if not SHA.fullmatch(commit):
        raise Unavailable("An exact 40-character commit is required")
    tree = git("rev-parse", commit + "^{tree}")
    parents = git("show", "-s", "--format=%P", commit).split()
    possible = [commit] + (parents if len(parents) == 2 else [])
    return [item for item in possible if git("rev-parse", item + "^{tree}") == tree]


def trusted_run(run, repository, commit, workflow_id):
    return (
        run.get("workflow_id") == workflow_id
        and run.get("head_sha") == commit
        and run.get("head_repository", {}).get("full_name") == repository
        and run.get("path", "").split("@")[0] == ".github/workflows/ci.yml"
        and run.get("event") in {"pull_request", "push", "workflow_dispatch"}
        and run.get("status") == "completed"
        and run.get("conclusion") == "success"
        and fresh(run.get("updated_at"))
    )


def validate_proof(proof, run, repository, commit, tree, artifacts):
    expected = {
        "schema": 1, "repository": repository, "source_commit": commit,
        "source_tree": tree, "platform": "linux/amd64",
        "run_id": run["id"], "run_attempt": run["run_attempt"],
        "image_tag": "codex-proxy-rs:ci-" + commit,
        "image_artifact": f"cpr-image-linux-amd64-{commit}-{run['run_attempt']}",
    }
    if any(proof.get(key) != value for key, value in expected.items()):
        raise Unavailable("CI image provenance does not match the requested source")
    if not DIGEST.fullmatch(proof.get("image_id", "")):
        raise Unavailable("Invalid image ID")
    if not re.fullmatch(r"[0-9a-f]{64}", proof.get("archive_sha256", "")):
        raise Unavailable("Invalid archive checksum")
    if not SHA.fullmatch(proof.get("migrations_tree", "")):
        raise Unavailable("Missing migration identity")
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.+-]+)?", proof.get("version", "")):
        raise Unavailable("Invalid image version")
    if not fresh(proof.get("created_at")):
        raise Unavailable("CI image validation has expired")
    matches = [a for a in artifacts if a["name"] == proof["image_artifact"] and not a["expired"]]
    if len(matches) != 1 or not DIGEST.fullmatch(matches[0].get("digest", "")):
        raise Unavailable("The immutable image artifact is unavailable")
    return matches[0]


def artifact_file(repository, artifact, member, destination):
    """Never extract arbitrary ZIP paths or trust only the manifest checksum."""
    if artifact.get("expired") or not DIGEST.fullmatch(artifact.get("digest", "")):
        raise Unavailable("Artifact lacks a usable GitHub digest")
    if artifact["size_in_bytes"] > 4 * 1024**3:
        raise Unavailable("Artifact exceeds the deployment size limit")
    destination = Path(destination)
    with tempfile.TemporaryDirectory(prefix="cpr-artifact-") as temporary:
        archive = Path(temporary) / "artifact.zip"
        with archive.open("wb") as output:
            subprocess.run([
                "gh", "api", f"repos/{repository}/actions/artifacts/{artifact['id']}/zip",
            ], stdout=output, check=True, timeout=600)
        if "sha256:" + sha256(archive) != artifact["digest"]:
            raise Unavailable("GitHub artifact digest mismatch")
        with zipfile.ZipFile(archive) as bundle:
            if bundle.namelist() != [member]:
                raise Unavailable("Unexpected artifact contents")
            limit = 1024**2 if member == "manifest.json" else 4 * 1024**3
            if bundle.getinfo(member).file_size > limit:
                raise Unavailable("Artifact member exceeds its size limit")
            with bundle.open(member) as source, destination.open("wb") as target:
                shutil.copyfileobj(source, target)


def resolve(repository, commit):
    if not REPOSITORY.fullmatch(repository):
        raise Unavailable("Invalid repository")
    workflow = api(f"repos/{repository}/actions/workflows/ci.yml")["id"]
    tree = git("rev-parse", commit + "^{tree}")
    for source in candidates(commit):
        runs = api(
            f"repos/{repository}/actions/workflows/{workflow}/runs"
            f"?head_sha={source}&per_page=20"
        )["workflow_runs"]
        # A newer failed/cancelled attempt must not be hidden by an older success.
        runs = [run for run in runs if run["event"] in {"pull_request", "push", "workflow_dispatch"}]
        if not runs:
            continue
        run = max(runs, key=lambda item: item["id"])
        if not trusted_run(run, repository, source, workflow):
            continue
        artifacts = api(f"repos/{repository}/actions/runs/{run['id']}/artifacts?per_page=100")["artifacts"]
        name = f"cpr-proof-{source}-{run['run_attempt']}"
        proofs = [a for a in artifacts if a["name"] == name and not a["expired"]]
        if len(proofs) != 1:
            continue
        try:
            with tempfile.TemporaryDirectory(prefix="cpr-proof-") as temporary:
                path = Path(temporary) / "manifest.json"
                artifact_file(repository, proofs[0], "manifest.json", path)
                proof = json.loads(path.read_text())
            image = validate_proof(proof, run, repository, source, tree, artifacts)
            if proof["migrations_tree"] != git("rev-parse", commit + ":backend/migrations"):
                raise Unavailable("Migration tree mismatch")
            return {"target_commit": commit, "proof": proof, "artifact": image}
        except (Unavailable, ValueError, KeyError, zipfile.BadZipFile):
            continue
    raise Unavailable("No matching verified image; run CI for the exact main commit first")


def require_security(repository, commit):
    workflow = api(f"repos/{repository}/actions/workflows/security-scan.yml")["id"]
    runs = api(
        f"repos/{repository}/actions/workflows/{workflow}/runs?head_sha={commit}&per_page=20"
    )["workflow_runs"]
    if not runs:
        raise Unavailable("Security checks are missing for the target commit")
    run = max(runs, key=lambda item: item["id"])
    if (run["head_repository"]["full_name"] != repository
            or run.get("head_sha") != commit or run.get("workflow_id") != workflow
            or run.get("path", "").split("@")[0] != ".github/workflows/security-scan.yml"
            or run["status"] != "completed" or run["conclusion"] != "success"
            or not fresh(run["updated_at"])):
        raise Unavailable("Latest target security checks have not passed")
    jobs = api(f"repos/{repository}/actions/runs/{run['id']}/jobs?filter=latest&per_page=100")["jobs"]
    passed = {job["name"] for job in jobs if job["conclusion"] == "success"}
    if not SECURITY_JOBS <= passed:
        raise Unavailable("A privacy-only run is not a complete security gate")


def create(image, archive, output):
    image_info = json.loads(command("docker", "image", "inspect", image))[0]
    source = git("rev-parse", "HEAD")
    labels = image_info["Config"]["Labels"]
    if labels["org.opencontainers.image.revision"] != source:
        raise Unavailable("Built image revision does not match the checkout")
    if image_info["Architecture"] != "amd64" or image_info["Os"] != "linux":
        raise Unavailable("Unsupported deployment platform")
    attempt = int(os.environ["GITHUB_RUN_ATTEMPT"])
    proof = {
        "schema": 1, "repository": os.environ["GITHUB_REPOSITORY"],
        "source_commit": source, "source_tree": git("rev-parse", "HEAD^{tree}"),
        "migrations_tree": git("rev-parse", "HEAD:backend/migrations"),
        "run_id": int(os.environ["GITHUB_RUN_ID"]), "run_attempt": attempt,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "platform": "linux/amd64", "version": labels["org.opencontainers.image.version"],
        "image_id": image_info["Id"], "image_tag": image,
        "archive_sha256": sha256(archive),
        "image_artifact": f"cpr-image-linux-amd64-{source}-{attempt}",
    }
    Path(output).write_text(json.dumps(proof, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    make = commands.add_parser("create")
    for name in ("image", "archive", "output"):
        make.add_argument("--" + name, required=True)
    find = commands.add_parser("resolve")
    for name in ("repository", "commit", "output"):
        find.add_argument("--" + name, required=True)
    find.add_argument("--optional", action="store_true")
    args = parser.parse_args()
    if args.command == "create":
        create(args.image, args.archive, args.output)
        return
    reused = False
    try:
        selected = resolve(args.repository, args.commit)
        Path(args.output).write_text(json.dumps(selected, indent=2) + "\n")
        reused = True
    except (Unavailable, subprocess.SubprocessError, OSError, ValueError, KeyError):
        if not args.optional:
            raise
        print("No reusable CI proof; normal validation and build will run.")
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"reused={str(reused).lower()}\n")
    elif not reused:
        raise Unavailable("No verified image found")


if __name__ == "__main__":
    main()
