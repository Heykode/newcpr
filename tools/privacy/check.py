#!/usr/bin/env python3
"""Read-only privacy checks. Never prints matching values or executes repository files."""

import argparse
from dataclasses import dataclass
import hashlib
import ipaddress
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tempfile


HERE = Path(__file__).resolve().parent
POLICY = json.loads((HERE / "policy.json").read_text(encoding="utf-8"))
REVIEWED_FILE = HERE / "reviewed-examples.json"
REVIEWED = json.loads(REVIEWED_FILE.read_text(encoding="utf-8")) if REVIEWED_FILE.exists() else []
HOME = re.compile(r"(?:/(?:Users|home)/|[A-Za-z]:[\\/]+Users[\\/]+)([^\s/\\\"'`<>]+)")
IPV4 = re.compile(r"(?<![\w.])(?:\d{1,3}\.){3}\d{1,3}(?![\w.])")
IPV6 = re.compile(r"(?<![\w:])(?:[a-fA-F0-9]{0,4}:){2,}[a-fA-F0-9:.]{0,45}(?![\w:])")
CONNECTION = re.compile(r"\b(?:postgres(?:ql)?|redis|mysql|mongodb(?:\+srv)?|https?)://[^\s\"'`<>]+")
PRIVATE_KEY = re.compile(r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----")
INFRA = re.compile(r"(?:sites-(?:available|enabled)/|server_name\s+|HostName\s+)([a-zA-Z0-9][a-zA-Z0-9.-]*\.[a-zA-Z]{2,})")
EMAIL = re.compile(r"<([^<>\s]+)>")
EXAMPLES = (
    ipaddress.ip_network("192.0.2.0/24"),
    ipaddress.ip_network("198.51.100.0/24"),
    ipaddress.ip_network("203.0.113.0/24"),
    ipaddress.ip_network("2001:db8::/32"),
)


class CheckError(Exception):
    pass


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    rule: str


def unreviewed(name, data, findings):
    digest = hashlib.sha256(data).hexdigest()
    lines = data.splitlines()
    allowed_rules = {"non-example-ip", "authenticated-url", "personal-home-path", "gitleaks:generic-api-key", "gitleaks:cpr-api-key"}
    accepted = set()
    for item in REVIEWED:
        if item["path"] != name or item["file_sha256"] != digest or item["rule"] not in allowed_rules:
            continue
        number = item["line"]
        if 0 < number <= len(lines) and hashlib.sha256(lines[number - 1]).hexdigest() == item["line_sha256"]:
            accepted.add((number, item["rule"]))
    return [f for f in findings if (f.line, f.rule) not in accepted]


def git(*args, optional=False):
    result = subprocess.run(["git", *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode and not (optional and result.returncode == 1):
        raise CheckError("Git inspection failed; fetch missing history or resolve the index.")
    return result.stdout


def root():
    return Path(os.fsdecode(git("rev-parse", "--show-toplevel")).strip()).resolve()


def blocklist(repo):
    configured = os.environ.get("PRIVACY_BLOCKLIST") or os.fsdecode(
        git("config", "--get", "privacy.blocklistPath", optional=True)
    ).strip()
    if not configured:
        return []
    source = Path(configured).expanduser().resolve()
    if source == repo or repo in source.parents:
        raise CheckError("The private blocklist must be outside the repository.")
    try:
        data = json.loads(source.read_text(encoding="utf-8"))
        values = data["literals"]
        if not isinstance(values, list) or any(not isinstance(x, str) or len(x) < 4 for x in values):
            raise ValueError()
        return [x.casefold() for x in values]
    except (OSError, ValueError, KeyError, TypeError):
        raise CheckError("Private blocklist is missing or invalid.") from None


def placeholder(value):
    return value in {"...", "user", "username", "example", "runner"} or value.startswith(("$", "{"))


def example_hostname(value):
    value = value.lower().rstrip(".")
    return value.endswith((".example", ".invalid", ".test")) or any(
        value == domain or value.endswith("." + domain)
        for domain in ("example.com", "example.org", "example.net")
    )


def content_findings(name, data, private_literals):
    if len(data) > POLICY["max_file_bytes"]:
        return [Finding(name, 0, "oversized-file")]
    try:
        text = data.decode("utf-8")
        if "\0" in text:
            raise ValueError()
    except (UnicodeDecodeError, ValueError):
        return [Finding(name, 0, "binary-needs-review")]
    findings = []
    for number, line in enumerate(text.splitlines(), 1):
        if any(value in line.casefold() for value in private_literals):
            findings.append(Finding(name, number, "private-blocklist"))
        if any(not placeholder(m.group(1)) for m in HOME.finditer(line)):
            findings.append(Finding(name, number, "personal-home-path"))
        if PRIVATE_KEY.search(line):
            findings.append(Finding(name, number, "private-key"))
        if any(not example_hostname(m.group(1)) for m in INFRA.finditer(line)):
            findings.append(Finding(name, number, "infrastructure-hostname"))
        for match in CONNECTION.finditer(line):
            authority = match.group().split("://", 1)[1].split("/", 1)[0]
            if "@" in authority and ":" in authority.rsplit("@", 1)[0]:
                credential = authority.rsplit("@", 1)[0].split(":", 1)[1]
                if credential and not credential.startswith(("$", "{", "<")):
                    findings.append(Finding(name, number, "authenticated-url"))
        for expression in (IPV4, IPV6):
            for match in expression.finditer(line):
                try:
                    address = ipaddress.ip_address(match.group())
                except ValueError:
                    continue
                if address.is_loopback or address.is_unspecified:
                    continue
                if any(address.version == net.version and address in net for net in EXAMPLES):
                    continue
                findings.append(Finding(name, number, "non-example-ip"))
    return findings


def path_findings(name, private_literals):
    findings = content_findings(name, name.encode("utf-8"), private_literals)
    parts = PurePosixPath(name).parts
    if not parts or name.startswith("/") or any(p in {"", "..", ".git"} for p in parts) or "\\" in name:
        return findings + [Finding(name, 0, "unsafe-path")]
    leaf = parts[-1]
    if (
        any(name.startswith(p) or "/" + p in name for p in POLICY["private_roots"])
        or any(p.casefold() in POLICY["private_components"] for p in parts)
        or name in POLICY["private_files"]
        or leaf in POLICY["private_files"]
        or leaf.startswith(".env.") and not leaf.endswith((".example", ".sample", ".template"))
        or any(leaf.lower().endswith(suffix) for suffix in POLICY["private_suffixes"])
    ):
        findings.append(Finding(name, 0, "private-file"))
    if name not in POLICY["public_files"] and not any(name.startswith(p) for p in POLICY["public_roots"]):
        findings.append(Finding(name, 0, "outside-public-allowlist"))
    return findings


def blob(oid):
    size = int(git("cat-file", "-s", oid))
    if size > POLICY["max_file_bytes"]:
        raise CheckError("An oversized Git object requires manual review.")
    return git("cat-file", "blob", oid)


def index_entries():
    changed = set(git("diff", "--cached", "--name-only", "--diff-filter=ACMRT", "-z").split(b"\0"))
    entries = []
    for row in git("ls-files", "--stage", "-z").split(b"\0"):
        if not row:
            continue
        header, path = row.split(b"\t", 1)
        mode, oid, stage = header.decode("ascii").split()
        if stage != "0":
            raise CheckError("Resolve index conflicts before committing.")
        if path in changed:
            entries.append((os.fsdecode(path), mode, oid))
    return entries


def tree_entries(ref):
    entries = []
    for row in git("ls-tree", "-rz", ref).split(b"\0"):
        if not row:
            continue
        header, path = row.split(b"\t", 1)
        mode, kind, oid = header.decode("ascii").split()
        if kind != "blob":
            raise CheckError("Submodules require a separate publication review.")
        entries.append((os.fsdecode(path), mode, oid))
    return entries


def identity_findings(label, value):
    addresses = EMAIL.findall(value)
    if not addresses or any(
        a.lower() != "noreply@github.com"
        and a.rpartition("@")[2].lower() not in POLICY["public_email_domains"]
        for a in addresses
    ):
        return [Finding(label, 0, "non-public-git-email")]
    return []


def reviewed_commit_identities():
    items = POLICY.get("reviewed_commit_identities", [])
    if not isinstance(items, list):
        raise CheckError("Reviewed commit identities are invalid.")
    commits = set()
    for item in items:
        if not isinstance(item, dict) or set(item) != {"commit", "reason"}:
            raise CheckError("Reviewed commit identities are invalid.")
        commit = item["commit"]
        reason = item["reason"]
        if (
            not isinstance(commit, str)
            or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit)
            or commit in commits
            or not isinstance(reason, str)
            or not reason.strip()
            or any(ord(char) < 32 for char in reason)
        ):
            raise CheckError("Reviewed commit identities are invalid.")
        commits.add(commit)
    return commits


def gitleaks(records):
    configured = os.environ.get("PRIVACY_GITLEAKS") or os.fsdecode(git("config", "--get", "privacy.gitleaksPath", optional=True)).strip()
    binary = configured or shutil.which("gitleaks")
    if not binary:
        raise CheckError("Gitleaks is required; install it before committing or publishing.")
    findings = []
    with tempfile.TemporaryDirectory(prefix="privacy-scan-") as directory:
        parent = Path(directory)
        source = parent / "scan"
        source.mkdir()
        labels = {}
        # Flat generated names prevent a candidate's ignore/config files from changing scan behavior.
        for index, (name, data) in enumerate(records):
            leaf = f"{index:06d}.txt"
            (source / leaf).write_bytes(data)
            labels[leaf] = (name, data)
        ignored = parent / "empty.ignore"
        ignored.write_text("", encoding="utf-8")
        report = parent / "report.json"
        command = [
            binary, "dir", str(source), "--config", str(HERE / "gitleaks.toml"),
            "--gitleaks-ignore-path", str(ignored), "--ignore-gitleaks-allow",
            "--redact=100", "--no-banner", "--no-color", "--report-format", "json",
            "--report-path", str(report), "--timeout", "120",
        ]
        try:
            result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=135)
            if result.returncode not in (0, 1) or not report.is_file():
                raise CheckError("Gitleaks failed; no content was approved.")
            entries = json.loads(report.read_text(encoding="utf-8"))
            if not isinstance(entries, list) or (result.returncode == 1 and not entries):
                raise ValueError()
            for entry in entries:
                if not isinstance(entry, dict) or not isinstance(entry.get("File"), str):
                    raise ValueError()
                if not isinstance(entry.get("RuleID"), str) or not re.fullmatch(r"[\w.-]+", entry["RuleID"]):
                    raise ValueError()
                if type(entry.get("StartLine")) is not int or entry["StartLine"] < 1:
                    raise ValueError()
                label, data = labels[Path(entry["File"]).name]
                finding = Finding(label, int(entry["StartLine"]), "gitleaks:" + entry["RuleID"])
                findings.extend(unreviewed(label, data, [finding]))
        except (OSError, subprocess.TimeoutExpired, ValueError, KeyError, TypeError):
            raise CheckError("Gitleaks could not complete a valid scan.") from None
    return findings


def scan(entries, private_literals, messages=()):
    records = []
    findings = []
    seen = set()
    for name, mode, oid in entries:
        if (name, oid) in seen:
            continue
        seen.add((name, oid))
        findings.extend(path_findings(name, private_literals))
        if mode not in {"100644", "100755"}:
            findings.append(Finding(name, 0, "link-or-special-file"))
            continue
        data = blob(oid)
        findings.extend(unreviewed(name, data, content_findings(name, data, private_literals)))
        records.append((name, data))
    for name, data in messages:
        findings.extend(content_findings(name, data, private_literals))
        records.append((name, data))
    findings.extend(gitleaks(records))
    return findings


def history(ref, exclude=None):
    if git("rev-parse", "--is-shallow-repository").strip() != b"false":
        raise CheckError("Complete history is required; shallow repositories cannot be approved.")
    target = git("rev-parse", "--verify", ref + "^{commit}").decode().strip()
    revisions = [target]
    if exclude:
        revisions.append("^" + git("rev-parse", "--verify", exclude + "^{commit}").decode().strip())
    entries, messages, findings = [], [], []
    commits = git("rev-list", *revisions).decode().splitlines()
    reviewed_identities = reviewed_commit_identities()
    for commit in commits:
        info = git("show", "-s", "--format=%an <%ae>%n%cn <%ce>%n%B", commit)
        if commit not in reviewed_identities:
            findings.extend(identity_findings("commit-" + commit[:12], "\n".join(info.decode("utf-8").splitlines()[:2])))
        messages.append(("commit-" + commit[:12], info))
        # Every reachable commit is included; a later deletion cannot hide an earlier leak.
        entries.extend(tree_entries(commit))
    tag_ref = ref
    while git("cat-file", "-t", tag_ref).strip() == b"tag":
        tag = git("cat-file", "tag", tag_ref)
        label = "tag-" + hashlib.sha256(tag).hexdigest()[:12]
        messages.append((label, tag))
        tagger = next((line for line in tag.decode("utf-8").splitlines() if line.startswith("tagger ")), "")
        findings.extend(identity_findings(label, tagger))
        tag_ref = tag.splitlines()[0].removeprefix(b"object ").decode("ascii")
        if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", tag_ref):
            raise CheckError("Invalid annotated tag object.")
    return entries, messages, findings


def display(findings, private_literals):
    for finding in sorted(set(findings), key=lambda f: (f.path, f.line, f.rule)):
        label = finding.path
        if content_findings("name", label.encode("utf-8"), private_literals) or any(ord(c) < 32 for c in label):
            label = "file-" + hashlib.sha256(label.encode("utf-8")).hexdigest()[:12]
        print(f"{label}:{finding.line}: {finding.rule}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest="mode", required=True)
    modes.add_parser("staged")
    modes.add_parser("pre-push")
    for mode in ("tree", "history"):
        command = modes.add_parser(mode)
        command.add_argument("ref", nargs="?", default="HEAD")
    command = modes.add_parser("text")
    command.add_argument("file")
    args = parser.parse_args()
    try:
        repo = root()
        private_literals = blocklist(repo)
        findings = []
        if args.mode == "staged":
            for variable in ("GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"):
                findings.extend(identity_findings("git-identity", git("var", variable).decode("utf-8")))
            findings.extend(scan(index_entries(), private_literals))
        elif args.mode == "tree":
            findings.extend(scan(tree_entries(args.ref), private_literals))
        elif args.mode == "history":
            entries, messages, extra = history(args.ref)
            findings.extend(extra)
            findings.extend(scan(entries, private_literals, messages))
        elif args.mode == "text":
            source = Path(args.file)
            if source.is_symlink() or not source.is_file() or source.stat().st_size > POLICY["max_file_bytes"]:
                raise CheckError("Draft is not a regular, reviewable text file.")
            findings.extend(scan([], private_literals, [("draft", source.read_bytes())]))
        else:
            entries, messages = [], []
            for line in sys.stdin:
                fields = line.split()
                if len(fields) != 4:
                    raise CheckError("Invalid pre-push input.")
                local_ref, local_oid, remote_ref, remote_oid = fields
                if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", local_oid) or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", remote_oid):
                    raise CheckError("Invalid push object identifier.")
                if set(local_oid) == {"0"}:
                    continue
                messages.append(("push-ref-names", (local_ref + "\n" + remote_ref).encode("utf-8")))
                new_entries, new_messages, extra = history(local_oid, None if set(remote_oid) == {"0"} else remote_oid)
                entries.extend(new_entries)
                messages.extend(new_messages)
                findings.extend(extra)
            findings.extend(scan(entries, private_literals, messages))
        display(findings, private_literals)
        if findings:
            print("Privacy check blocked. Review locally; do not paste original values into reports.", file=sys.stderr)
            return 1
        print("Privacy check passed for the requested scope.")
        return 0
    except (CheckError, OSError, UnicodeError, ValueError) as error:
        message = str(error) if isinstance(error, CheckError) else "Input could not be inspected safely."
        print("Privacy check failed closed: " + message, file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
