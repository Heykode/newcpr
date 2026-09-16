import importlib.util
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("privacy_check", HERE / "check.py")
guard = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = guard
spec.loader.exec_module(guard)
HOME_PATH = "/" + "Users" + "/private-person/records"
PASSWORD_URL = "postgres" + "://tester:private-fixture@localhost/testing"
TOKEN = "gh" + "p_" + ("aB9qX4vN8rT2mK7sP5wE1yH6uC3dF0jL9zR2")


class ContentTests(unittest.TestCase):
    def test_github_merge_identity_is_public_but_other_mailboxes_are_not(self):
        self.assertEqual(guard.identity_findings("commit", "GitHub <noreply@github.com>"), [])
        self.assertTrue(guard.identity_findings("commit", "User <person@github.com>"))
        self.assertTrue(guard.identity_findings("commit", "User <noreply@notgithub.com>"))

    def reviewed(self, data, rule="authenticated-url"):
        return [{
            "path": "README.md", "line": 1, "rule": rule,
            "file_sha256": hashlib.sha256(data).hexdigest(),
            "line_sha256": hashlib.sha256(data.splitlines()[0]).hexdigest(),
            "reason": "Synthetic regression fixture",
        }]

    def test_reviewed_example_requires_exact_file_and_line(self):
        data = PASSWORD_URL.encode()
        finding = guard.Finding("README.md", 1, "authenticated-url")
        with patch.object(guard, "REVIEWED", self.reviewed(data)):
            self.assertEqual(guard.unreviewed("README.md", data, [finding]), [])
            self.assertEqual(guard.unreviewed("README.md", data + b"\nnew content", [finding]), [finding])
            self.assertEqual(guard.unreviewed("docs/other.md", data, [finding]), [finding])

    def test_private_blocklist_cannot_be_exempted(self):
        data = b"private-marker"
        finding = guard.Finding("README.md", 1, "private-blocklist")
        with patch.object(guard, "REVIEWED", self.reviewed(data, "private-blocklist")):
            self.assertEqual(guard.unreviewed("README.md", data, [finding]), [finding])

    def rules(self, text, literals=()):
        return {f.rule for f in guard.content_findings("README.md", text.encode(), literals)}

    def test_private_home_paths(self):
        self.assertIn("personal-home-path", self.rules(HOME_PATH))
        self.assertIn("personal-home-path", self.rules("C:\\" + "Users\\private-person\\records"))

    def test_example_and_loopback_addresses(self):
        sample = ".".join(["192", "0", "2", "11"])
        self.assertEqual(self.rules(sample + " localhost 127.0.0.1 ::1 0.0.0.0"), set())

    def test_real_and_private_addresses(self):
        for parts in (["10", "22", "33", "44"], ["8", "8", "4", "4"]):
            self.assertIn("non-example-ip", self.rules(".".join(parts)))
        self.assertIn("non-example-ip", self.rules("fd12" + ":3456::99"))

    def test_connection_and_private_key(self):
        self.assertIn("authenticated-url", self.rules(PASSWORD_URL))
        self.assertIn("private-key", self.rules("-----BEGIN " + "RSA PRIVATE KEY-----"))

    def test_hostname_embedded_in_config_path(self):
        location = "/etc/nginx/sites-" + "available/private-service.internal"
        self.assertIn("infrastructure-hostname", self.rules(location))
        self.assertNotIn("infrastructure-hostname", self.rules("server_name " + "service.example.com"))

    def test_private_blocklist(self):
        self.assertIn("private-blocklist", self.rules("HOST=private-node.example.invalid", ["private-node.example.invalid"]))

    def test_example_hostname_boundary(self):
        self.assertIn("infrastructure-hostname", self.rules("server_name " + "notexample.com"))
        self.assertNotIn("infrastructure-hostname", self.rules("HostName " + "EXAMPLE.COM"))

    def test_nested_private_directories(self):
        for component in guard.POLICY["private_components"]:
            self.assertIn("private-file", {
                f.rule for f in guard.path_findings("backend/" + component + "/record.txt", [])
            })

    def test_private_file_and_public_allowlist(self):
        for path in (".env", "deploy/config.yaml", ".trellis/tasks/test/prd.md",
                     "backend/.trellis/tasks/test/prd.md", "outputs/note.md",
                     "backend/local.db", "../escape"):
            self.assertTrue(guard.path_findings(path, []), path)
        for path in ("backend/src/lib.rs", ".trellis/spec/backend/index.md", "deploy/config.example.yaml", "AGENTS.md"):
            self.assertEqual(guard.path_findings(path, []), [], path)

    def test_binary_rejected(self):
        self.assertEqual(guard.content_findings("data", b"\0\1", [])[0].rule, "binary-needs-review")

    def test_report_never_prints_private_filename(self):
        stream = io.StringIO()
        with redirect_stderr(stream):
            guard.display([guard.Finding(HOME_PATH, 7, "personal-home-path")], [])
        self.assertNotIn(HOME_PATH, stream.getvalue())
        self.assertIn("file-", stream.getvalue())


class RepositoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.gitleaks = os.environ.get("PRIVACY_GITLEAKS") or shutil.which("gitleaks")
        if not cls.gitleaks:
            raise RuntimeError("Real Gitleaks is required for the integration tests.")

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="privacy-test-")
        self.repo = Path(self.temp.name)
        self.env = {
            k: v for k, v in os.environ.items()
            if not k.startswith(("GIT_", "PRIVACY_"))
        }
        self.env.update({"PRIVACY_GITLEAKS": self.gitleaks, "PYTHONDONTWRITEBYTECODE": "1"})
        self.run_git("init", "-q")
        self.run_git("config", "user.name", "Privacy Test")
        self.run_git("config", "user.email", "privacy-test@users.noreply.github.com")
        self.write("README.md", "Public fixture.\n")
        self.run_git("add", "README.md")
        self.run_git("commit", "-qm", "Initial public fixture")

    def tearDown(self):
        self.temp.cleanup()

    def run_git(self, *args):
        return subprocess.run(["git", *args], cwd=self.repo, env=self.env, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout.decode()

    def write(self, name, text):
        file = self.repo / name
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text, encoding="utf-8")
        return file

    def check(self, *args, input=None, env=None):
        return subprocess.run([sys.executable, str(HERE / "check.py"), *args], cwd=self.repo, env=env or self.env, input=input, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    def test_safe_staged_changes_pass(self):
        self.write("README.md", "Public update.\n")
        self.run_git("add", "README.md")
        result = self.check("staged")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_staged_secret_cannot_be_hidden_by_working_tree(self):
        self.write("README.md", HOME_PATH + "\n")
        self.run_git("add", "README.md")
        self.write("README.md", "Clean unstaged replacement.\n")
        result = self.check("staged")
        self.assertEqual(result.returncode, 1)
        self.assertIn("personal-home-path", result.stderr)
        self.assertNotIn(HOME_PATH, result.stderr)

    def test_unstaged_private_text_not_confused_with_staged_contents(self):
        self.write("README.md", "Staged public update.\n")
        self.run_git("add", "README.md")
        self.write("README.md", HOME_PATH)
        self.assertEqual(self.check("staged").returncode, 0)

    def test_real_gitleaks_blocks_token_without_echo(self):
        self.write("README.md", "token=" + TOKEN + "\n")
        self.run_git("add", "README.md")
        result = self.check("staged")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("gitleaks:", result.stderr)
        self.assertNotIn(TOKEN, result.stderr + result.stdout)

    def test_custom_session_token_is_blocked(self):
        value = "sess" + "_" + "aB9qX4vN8rT2mK7sP5wE1yH6uC3dF0jL"
        draft = self.write("draft.txt", value)
        result = self.check("text", str(draft))
        self.assertEqual(result.returncode, 1)
        self.assertIn("gitleaks:cpr-session-token", result.stderr)
        self.assertNotIn(value, result.stderr)

    def test_allow_comment_does_not_bypass_scan(self):
        draft = self.write("draft.txt", "token=" + TOKEN + " # gitleaks:allow\n")
        self.assertEqual(self.check("text", str(draft)).returncode, 1)

    def test_draft_is_scanned_separately(self):
        draft = self.write("draft.txt", PASSWORD_URL)
        result = self.check("text", str(draft))
        self.assertEqual(result.returncode, 1)
        self.assertNotIn(PASSWORD_URL, result.stderr)

    def test_git_email_checked(self):
        self.run_git("config", "user.email", "private-person@laptop.local")
        self.assertIn("non-public-git-email", self.check("staged").stderr)

    def test_deleted_secret_still_blocks_history_and_push(self):
        self.write("docs/private-note.md", HOME_PATH)
        self.run_git("add", "docs/private-note.md")
        self.run_git("commit", "-qm", "Temporary record")
        self.run_git("rm", "docs/private-note.md")
        self.run_git("commit", "-qm", "Remove record")
        self.assertEqual(self.check("tree").returncode, 0)
        self.assertEqual(self.check("history").returncode, 1)
        head = self.run_git("rev-parse", "HEAD").strip()
        update = f"refs/heads/main {head} refs/heads/main {'0' * 40}\n"
        self.assertEqual(self.check("pre-push", input=update).returncode, 1)

    def test_commit_message_is_scanned(self):
        self.run_git("commit", "--allow-empty", "-qm", HOME_PATH)
        self.assertEqual(self.check("history").returncode, 1)

    def test_annotated_tag_message_is_scanned(self):
        self.run_git("tag", "-a", "v-test", "-m", HOME_PATH)
        self.assertEqual(self.check("history", "v-test").returncode, 1)

    def test_nested_annotated_tag_message_is_scanned(self):
        self.run_git("tag", "-a", "inner", "-m", HOME_PATH)
        self.run_git("tag", "-a", "outer", "inner", "-m", "Public outer tag")
        self.assertEqual(self.check("history", "outer").returncode, 1)

    def test_shallow_history_fails_closed(self):
        head = self.run_git("rev-parse", "HEAD").strip()
        self.write(".git/shallow", head + "\n")
        result = self.check("history")
        self.assertEqual(result.returncode, 2)
        self.assertIn("shallow", result.stderr)

    def test_invalid_push_input_and_missing_history_fail_closed(self):
        head = self.run_git("rev-parse", "HEAD").strip()
        for update in ("invalid\n", f"refs/heads/main invalid refs/heads/main {'0' * 40}\n",
                       f"refs/heads/main {head} refs/heads/main {'a' * 40}\n"):
            self.assertEqual(self.check("pre-push", input=update).returncode, 2)

    def test_private_push_ref_name_is_scanned(self):
        head = self.run_git("rev-parse", "HEAD").strip()
        update = f"refs/heads/{HOME_PATH.lstrip('/')} {head} refs/heads/main {'0' * 40}\n"
        self.assertEqual(self.check("pre-push", input=update).returncode, 1)

    def test_public_tree_rejects_internal_records(self):
        self.write(".trellis/tasks/private/prd.md", "Even a harmless task is local only.\n")
        self.run_git("add", ".trellis/tasks/private/prd.md")
        self.assertIn("private-file", self.check("staged").stderr)

    def test_symlink_never_followed(self):
        (self.repo / "docs").mkdir()
        (self.repo / "docs/link").symlink_to(HERE / "check.py")
        self.run_git("add", "docs/link")
        self.assertIn("link-or-special-file", self.check("staged").stderr)

    def test_missing_scanner_fails_closed(self):
        env = {**self.env, "PRIVACY_GITLEAKS": str(self.repo / "missing-executable")}
        self.assertEqual(self.check("staged", env=env).returncode, 2)

    def test_crashing_scanner_fails_closed_without_echo(self):
        scanner = self.write("broken-scanner", "#!/bin/sh\nprintf private-error >&2\nexit 2\n")
        scanner.chmod(0o700)
        env = {**self.env, "PRIVACY_GITLEAKS": str(scanner)}
        result = self.check("staged", env=env)
        self.assertEqual(result.returncode, 2)
        self.assertNotIn("private-error", result.stderr)

    def test_blocklist_inside_repo_rejected(self):
        private = self.write("rules.json", '{"literals":["private-node.example.invalid"]}')
        result = self.check("staged", env={**self.env, "PRIVACY_BLOCKLIST": str(private)})
        self.assertEqual(result.returncode, 2)

    def test_invalid_scanner_reports_fail_closed_without_echo(self):
        for payload in ("null", "[null]", '[{"File":true}]', "[]"):
            with self.subTest(payload=payload):
                scanner = self.write("invalid-scanner", "#!/bin/sh\n"
                                     'while [ "$1" != "--report-path" ]; do shift; done\n'
                                     f"printf '%s' '{payload}' > \"$2\"\nexit 1\n")
                scanner.chmod(0o700)
                result = self.check("staged", env={**self.env, "PRIVACY_GITLEAKS": str(scanner)})
                self.assertEqual(result.returncode, 2)
                self.assertNotIn("Traceback", result.stderr)

    def test_external_blocklist_applies_to_paths_and_text(self):
        with tempfile.TemporaryDirectory(prefix="privacy-rules-") as directory:
            private = Path(directory) / "rules.json"
            private.write_text(json.dumps({"literals": ["private-node.example.invalid"]}))
            self.write("docs/private-node.example.invalid.md", "Public-looking text.")
            self.run_git("add", "docs")
            result = self.check("staged", env={**self.env, "PRIVACY_BLOCKLIST": str(private)})
            self.assertEqual(result.returncode, 1)
            self.assertNotIn("private-node.example.invalid", result.stderr)

    def test_local_config_blocklist_applies_without_environment(self):
        with tempfile.TemporaryDirectory(prefix="privacy-local-rules-") as directory:
            private = Path(directory) / "rules.json"
            private.write_text(json.dumps({"literals": ["private-node.example.invalid"]}))
            self.run_git("config", "--local", "privacy.blocklistPath", str(private))
            self.write("README.md", "private-node.example.invalid")
            self.run_git("add", "README.md")
            result = self.check("staged")
            self.assertEqual(result.returncode, 1)
            self.assertNotIn("private-node.example.invalid", result.stderr)

    def prepare_hooks(self):
        shutil.copytree(HERE, self.repo / "tools/privacy", ignore=shutil.ignore_patterns("__pycache__"))
        shutil.copytree(HERE.parent.parent / ".githooks", self.repo / ".githooks")
        return self.repo / "tools/privacy/install.py"

    def install(self):
        return subprocess.run([sys.executable, str(self.repo / "tools/privacy/install.py"), "--gitleaks", self.gitleaks], cwd=self.repo, env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    def test_hook_installation_backups_and_blocks_real_commit(self):
        self.prepare_hooks()
        before = (self.repo / ".git/config").read_bytes()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        backups = list((self.repo / ".git/privacy-hook-backups").glob("*/config"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), before)
        self.assertEqual(self.install().returncode, 0)
        self.assertEqual(len(list((self.repo / ".git/privacy-hook-backups").glob("*/config"))), 1)
        self.write("README.md", HOME_PATH)
        self.run_git("add", "README.md")
        commit = subprocess.run(["git", "commit", "-m", "Attempt"], cwd=self.repo, env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertNotEqual(commit.returncode, 0)
        self.assertIn(b"personal-home-path", commit.stderr)

    def test_existing_hook_configuration_is_not_overwritten(self):
        self.prepare_hooks()
        self.run_git("config", "core.hooksPath", "custom-hooks")
        before = (self.repo / ".git/config").read_bytes()
        self.assertEqual(self.install().returncode, 2)
        self.assertEqual((self.repo / ".git/config").read_bytes(), before)

    def test_shared_worktrees_are_not_modified(self):
        self.prepare_hooks()
        with tempfile.TemporaryDirectory(prefix="privacy-worktree-") as directory:
            self.run_git("worktree", "add", "--detach", str(Path(directory) / "other"))
            before = (self.repo / ".git/config").read_bytes()
            self.assertEqual(self.install().returncode, 2)
            self.assertEqual((self.repo / ".git/config").read_bytes(), before)

    def test_default_hooks_are_not_overwritten(self):
        self.prepare_hooks()
        hook = self.write(".git/hooks/pre-commit", "#!/bin/sh\nexit 0\n")
        hook.chmod(0o700)
        before = (self.repo / ".git/config").read_bytes()
        self.assertEqual(self.install().returncode, 2)
        self.assertEqual((self.repo / ".git/config").read_bytes(), before)
        self.assertEqual(hook.read_text(), "#!/bin/sh\nexit 0\n")

    def test_symlinked_config_is_not_modified(self):
        self.prepare_hooks()
        config = self.repo / ".git/config"
        original = config.read_bytes()
        moved = self.repo / ".git/config-original"
        config.rename(moved)
        config.symlink_to(moved)
        self.assertEqual(self.install().returncode, 2)
        self.assertEqual(moved.read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
