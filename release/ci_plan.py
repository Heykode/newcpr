"""Select conservative backend checks; missing evidence means the full workspace."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent.parent
PACKAGE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]*")
SHA = re.compile(r"[0-9a-f]{40}")
CONTROL_PATHS = {
    ".github/workflows/ci.yml", ".github/workflows/_quality.yml",
    ".github/workflows/_container.yml", "release/ci_plan.py",
    "release/build_ci_backend.sh", "release/version.yaml", "deploy/Dockerfile",
}
# Only localized administration/presentation changes use the narrow lane. New
# production surfaces and request/identity/storage owners remain full by default.
LOCAL_SURFACES = {
    "gateway-admin": (
        "src/use_case/observability", "src/use_case/notifications",
        "src/use_case/log_cleanup", "src/use_case/group_monitor",
        "src/use_case/request_capture",
    ),
    "gateway-api": (
        "src/admin/observability", "src/admin/notifications",
        "src/admin/log_cleanup", "src/admin/group_monitor",
        "src/admin/request_capture", "src/admin/accounts/presenter",
    ),
    "gateway-host": ("src/logging", "src/notifications", "src/client_distribution"),
}


def workspace(root):
    """Read normal, build, dev and target-specific local dependency edges."""
    backend = root / "backend"
    manifest = tomllib.loads((backend / "Cargo.toml").read_text())
    members = manifest["workspace"]["members"]
    packages = {}
    documents = {}
    for member in members:
        if any(char in member for char in "*?[!"):
            raise ValueError("Workspace globs require a reviewed planner update")
        directory = (backend / member).resolve()
        directory.relative_to(backend.resolve())
        document = tomllib.loads((directory / "Cargo.toml").read_text())
        name = document["package"]["name"]
        if not PACKAGE_NAME.fullmatch(name) or name in packages:
            raise ValueError("Invalid or duplicate workspace package")
        packages[name] = directory.relative_to(root.resolve()).as_posix()
        documents[name] = document
    by_directory = {(root / path).resolve(): name for name, path in packages.items()}
    dependencies = {name: set() for name in packages}
    for name, document in documents.items():
        sections = [document, *document.get("target", {}).values()]
        for section in sections:
            for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                for dependency, spec in section.get(kind, {}).items():
                    if not isinstance(spec, dict):
                        continue
                    directory = root / packages[name]
                    if spec.get("workspace"):
                        spec = manifest["workspace"]["dependencies"][dependency]
                        directory = backend
                    if not isinstance(spec, dict) or "path" not in spec:
                        continue
                    target = (directory / spec["path"]).resolve()
                    if target not in by_directory:
                        raise ValueError("Local dependency is not a workspace member")
                    dependencies[name].add(by_directory[target])
    return packages, dependencies


def reverse_closure(changed, dependencies):
    selected = set(changed)
    while True:
        expanded = selected | {
            name for name, inputs in dependencies.items() if inputs & selected
        }
        if expanded == selected:
            return sorted(selected)
        selected = expanded


def full_plan(reason):
    return {"backend_packages": [], "redis_acl": True, "reason": reason}


def backend_plan(paths, root=ROOT):
    if paths is None or not paths or CONTROL_PATHS.intersection(paths):
        return full_plan("Full validation requested or comparison/control inputs changed")
    try:
        packages, dependencies = workspace(root)
    except (OSError, ValueError, KeyError, TypeError):
        return full_plan("Workspace graph could not be resolved")
    changed = set()
    for path in paths:
        if not path.startswith("backend/"):
            continue
        owners = [(name, prefix) for name, prefix in packages.items()
                  if path.startswith(prefix + "/")]
        if len(owners) != 1:
            return full_plan("Shared or unknown backend input changed")
        name, prefix = owners[0]
        relative = path[len(prefix) + 1:]
        local = any(relative == surface + ".rs" or relative.startswith(surface + "/")
                    for surface in LOCAL_SURFACES.get(name, ()))
        tests_only = relative.startswith("tests/")
        if not (local or tests_only):
            return full_plan("Critical or unclassified backend surface changed")
        changed.add(name)
    if not changed:
        return full_plan("No localized backend owner found")
    selected = reverse_closure(changed, dependencies)
    # The application also owns architecture and cross-package assembly tests.
    if "codex-proxy-rs" not in selected:
        selected.append("codex-proxy-rs")
    if "codex-proxy-rs" not in packages:
        return full_plan("Application architecture test owner is missing")
    return {"backend_packages": sorted(selected),
            "redis_acl": "gateway-store" in selected,
            "reason": "Changed packages, reverse dependencies and application tests"}


def changed_paths(base, head, event, root=ROOT):
    if event == "workflow_dispatch" or not SHA.fullmatch(base or "") or base == "0" * 40:
        return None
    if not SHA.fullmatch(head or ""):
        return None
    try:
        if event == "pull_request":
            base = subprocess.check_output(
                ["git", "merge-base", base, head], cwd=root, text=True, timeout=30,
                stderr=subprocess.DEVNULL,
            ).strip()
        output = subprocess.check_output(
            ["git", "diff", "--no-renames", "--name-only", "-z", base, head, "--"],
            cwd=root, timeout=30, stderr=subprocess.DEVNULL,
        )
        return [path.decode("utf-8") for path in output.split(b"\0") if path]
    except (subprocess.SubprocessError, OSError, UnicodeError):
        return None


def cargo_arguments(packages, root=ROOT):
    names = json.loads(packages)
    if not isinstance(names, list) or any(
        not isinstance(name, str) or not PACKAGE_NAME.fullmatch(name) for name in names
    ):
        raise ValueError("Invalid Cargo package selection")
    if not names:
        return ["--workspace"]
    available, _ = workspace(root)
    if not set(names) <= available.keys() or len(names) != len(set(names)):
        raise ValueError("Unknown or duplicate Cargo package")
    return [arg for name in names for arg in ("--package", name)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    plan = commands.add_parser("plan")
    plan.add_argument("--base", default="")
    plan.add_argument("--head", required=True)
    plan.add_argument("--event", required=True)
    cargo = commands.add_parser("cargo")
    cargo.add_argument("--packages", default="[]")
    cargo.add_argument("operation", choices=("clippy", "test"))
    cargo.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command == "plan":
        result = backend_plan(changed_paths(args.base, args.head, args.event))
        print(json.dumps(result, sort_keys=True))
        if os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a") as output:
                for name in ("backend_packages", "redis_acl"):
                    output.write(name + "=" + json.dumps(result[name], separators=(",", ":")) + "\n")
        return
    arguments = args.arguments
    if arguments and arguments[0] == "--":
        arguments = arguments[1:]
    command = ["cargo", args.operation, *cargo_arguments(args.packages), *arguments]
    print(json.dumps(command), flush=True)
    subprocess.run(command, cwd=ROOT / "backend", check=True)


if __name__ == "__main__":
    main()
