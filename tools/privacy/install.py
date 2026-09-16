#!/usr/bin/env python3
"""Install local privacy hooks only after checking existing hooks and backing up Git config."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def git(*args, optional=False):
    result = subprocess.run(["git", *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if result.returncode and not (optional and result.returncode == 1):
        raise RuntimeError("Git configuration could not be inspected safely.")
    return result.stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gitleaks", required=True, help="Path to a locally verified Gitleaks executable")
    args = parser.parse_args()
    try:
        repo = Path(git("rev-parse", "--show-toplevel")).resolve()
        os.chdir(repo)
        if sum(line.startswith("worktree ") for line in git("worktree", "list", "--porcelain").splitlines()) != 1:
            raise RuntimeError("Shared worktrees detected; coordinate hook installation separately.")
        binary = Path(args.gitleaks).expanduser().resolve()
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise RuntimeError("A verified executable Gitleaks path is required.")
        result = subprocess.run([str(binary), "version"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        if result.returncode:
            raise RuntimeError("Gitleaks version check failed.")
        hooks = repo / ".githooks"
        if hooks.is_symlink():
            raise RuntimeError("Symlinked hook directories require manual review.")
        for name in ("pre-commit", "commit-msg", "pre-push"):
            file = hooks / name
            if file.is_symlink() or not file.is_file() or not os.access(file, os.X_OK):
                raise RuntimeError("Privacy hook files are missing or not executable.")
        configured = git("config", "--local", "--get", "core.hooksPath", optional=True)
        effective = git("config", "--get", "core.hooksPath", optional=True)
        if effective and Path(effective).resolve() != hooks:
            raise RuntimeError("Existing custom hooks detected; refusing to replace them.")
        default_hooks = Path(git("rev-parse", "--git-path", "hooks"))
        if not effective and default_hooks.exists() and any(p.is_file() and not p.name.endswith(".sample") for p in default_hooks.iterdir()):
            raise RuntimeError("Existing Git hooks detected; integrate them manually.")
        if configured == str(hooks) and git("config", "--local", "--get", "privacy.gitleaksPath", optional=True) == str(binary):
            print("Privacy hooks already configured; no changes made.")
            return 0
        config = Path(git("rev-parse", "--git-path", "config"))
        if config.is_symlink() or not config.is_file():
            raise RuntimeError("Git config must be a regular non-symlinked file.")
        config = config.resolve()
        backup_root = config.parent / "privacy-hook-backups"
        if backup_root.is_symlink():
            raise RuntimeError("Symlinked backup directories are not allowed.")
        backup_root.mkdir(mode=0o700, exist_ok=True)
        backup = Path(tempfile.mkdtemp(prefix="install-", dir=backup_root))
        shutil.copy2(config, backup / "config")
        (backup / "config").chmod(0o600)
        git("config", "--local", "privacy.gitleaksPath", str(binary))
        git("config", "--local", "core.hooksPath", str(hooks))
        print("Privacy hooks installed locally; previous Git config backed up inside the Git directory.")
        print("Git author/email settings were not changed. Verify a public noreply identity before committing.")
        return 0
    except (RuntimeError, OSError, subprocess.TimeoutExpired) as error:
        message = str(error) if isinstance(error, RuntimeError) else "Installation could not complete safely."
        print("Hook installation stopped: " + message, file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
