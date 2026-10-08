import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("ci_plan", ROOT / "release/ci_plan.py")
plan = importlib.util.module_from_spec(spec)
spec.loader.exec_module(plan)


class BackendPlanTests(unittest.TestCase):
    def test_api_presentation_includes_reverse_dependencies_and_architecture(self):
        result = plan.backend_plan(["backend/crates/gateway-api/src/admin/accounts/presenter.rs"])
        self.assertEqual(result["backend_packages"], ["codex-proxy-rs", "gateway-api"])
        self.assertFalse(result["redis_acl"])

    def test_admin_change_covers_store_providers_and_all_consumers(self):
        result = plan.backend_plan(["backend/crates/gateway-admin/src/use_case/log_cleanup.rs"])
        self.assertEqual(result["backend_packages"], [
            "codex-proxy-rs", "gateway-admin", "gateway-api", "gateway-host",
            "gateway-store", "provider-openai", "provider-xai",
        ])
        self.assertTrue(result["redis_acl"])

    def test_host_logging_covers_application(self):
        result = plan.backend_plan(["backend/crates/gateway-host/src/logging/sink.rs"])
        self.assertEqual(result["backend_packages"], ["codex-proxy-rs", "gateway-host"])
        self.assertFalse(result["redis_acl"])

    def test_multiple_local_changes_union_their_consumers(self):
        result = plan.backend_plan([
            "frontend/src/views/AccountsView.vue",
            "backend/crates/gateway-host/src/logging/sink.rs",
            "backend/crates/gateway-api/src/admin/observability/query.rs",
        ])
        self.assertEqual(result["backend_packages"], ["codex-proxy-rs", "gateway-api", "gateway-host"])

    def test_test_only_changes_include_owners_and_consumers(self):
        result = plan.backend_plan(["backend/crates/providers/openai/tests/transport/headers.rs"])
        self.assertEqual(result["backend_packages"], ["codex-proxy-rs", "provider-openai"])

    def test_critical_and_unknown_paths_require_full_checks(self):
        paths = [
            "backend/crates/gateway-core/src/engine/scheduler.rs",
            "backend/crates/gateway-protocol/src/openai.rs",
            "backend/crates/gateway-store/src/redis/mod.rs",
            "backend/crates/providers/openai/src/transport/headers.rs",
            "backend/crates/providers/xai/src/provider.rs",
            "backend/crates/gateway-api/src/openai/responses/http.rs",
            "backend/crates/gateway-api/src/admin/auth.rs",
            "backend/crates/gateway-api/src/admin/accounts/credentials.rs",
            "backend/crates/gateway-admin/src/model/accounts.rs",
            "backend/crates/gateway-admin/src/ports/store.rs",
            "backend/crates/gateway-admin/src/use_case/relogin/worker.rs",
            "backend/crates/gateway-admin/src/use_case/user_agent.rs",
            "backend/crates/gateway-admin/src/use_case/new_surface.rs",
            "backend/crates/gateway-host/src/config.rs",
            "backend/crates/gateway-host/src/serve.rs",
            "backend/apps/gateway/src/main.rs",
            "backend/migrations/0001_initial.sql", "backend/migrations/.frozen-sha256",
            "backend/new-package/src/lib.rs", "backend/Cargo.lock", "backend/Cargo.toml",
            "backend/crates/gateway-api/Cargo.toml", "backend/.cargo/config.toml",
            "backend/rust-toolchain.toml", "backend/crates/gateway-host/build.rs",
            "backend/crates/gateway-host/src/logging-unsafe.rs",
            *plan.CONTROL_PATHS,
        ]
        for path in paths:
            with self.subTest(path=path):
                result = plan.backend_plan([
                    "backend/crates/gateway-api/src/admin/observability/query.rs", path,
                ])
                self.assertEqual(result["backend_packages"], [])
                self.assertTrue(result["redis_acl"])

    def test_missing_evidence_never_silently_skips_backend(self):
        for paths in (None, [], ["frontend/src/main.ts"]):
            self.assertEqual(plan.backend_plan(paths)["backend_packages"], [])
        with patch.object(plan, "workspace", side_effect=ValueError("unknown manifest")):
            self.assertEqual(plan.backend_plan(["backend/crates/gateway-api/tests/main.rs"])["backend_packages"], [])

    def test_reverse_dependencies_are_transitive_and_cycles_terminate(self):
        graph = {"base": set(), "middle": {"base"}, "app": {"middle"}, "cycle": {"app"}}
        graph["app"].add("cycle")
        self.assertEqual(plan.reverse_closure({"base"}, graph), ["app", "base", "cycle", "middle"])

    def test_cargo_package_arguments_are_validated_not_shell_expanded(self):
        self.assertEqual(plan.cargo_arguments("[]"), ["--workspace"])
        self.assertEqual(plan.cargo_arguments('["gateway-api","codex-proxy-rs"]'),
                         ["--package", "gateway-api", "--package", "codex-proxy-rs"])
        for value in ('null', '{}', '"gateway-api"', '["--workspace"]',
                      '["gateway-api; false"]', '["missing"]', '[1]',
                      '["gateway-api", "gateway-api"]'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                plan.cargo_arguments(value)


class GraphTests(unittest.TestCase):
    def test_target_build_dev_renamed_and_workspace_dependencies(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            backend = root / "backend"
            backend.mkdir()
            (backend / "Cargo.toml").write_text(
                '[workspace]\nmembers=["base","consumer"]\n'
                '[workspace.dependencies]\nshared={path="base"}\n'
            )
            for member in ("base", "consumer"):
                (backend / member).mkdir()
                (backend / member / "Cargo.toml").write_text('[package]\nname="' + member + '"\n')
            for extra in (
                '[dev-dependencies]\nrenamed={path="../base",package="base"}\n',
                '[build-dependencies]\nbase={path="../base"}\n',
                '[target.\'cfg(unix)\'.dependencies]\nbase={path="../base"}\n',
                '[dependencies]\nshared={workspace=true}\n',
            ):
                with self.subTest(extra=extra):
                    (backend / "consumer/Cargo.toml").write_text('[package]\nname="consumer"\n' + extra)
                    packages, graph = plan.workspace(root)
                    self.assertEqual(packages["base"], "backend/base")
                    self.assertEqual(graph["consumer"], {"base"})
            (backend / "consumer/Cargo.toml").write_text(
                '[package]\nname="consumer"\n[dependencies]\nmissing={path="../absent"}\n'
            )
            with self.assertRaises(ValueError):
                plan.workspace(root)


class GitComparisonTests(unittest.TestCase):
    def test_manual_missing_or_unresolvable_base_falls_back_to_full(self):
        for base, head, event in (
            ("a" * 40, "b" * 40, "workflow_dispatch"),
            ("0" * 40, "b" * 40, "push"), ("", "b" * 40, "push"),
            ("bad", "b" * 40, "pull_request"), ("a" * 40, "bad", "push"),
        ):
            self.assertIsNone(plan.changed_paths(base, head, event))
        with patch.object(subprocess, "check_output", side_effect=subprocess.CalledProcessError(1, "git")):
            self.assertIsNone(plan.changed_paths("a" * 40, "b" * 40, "push"))

    def test_diff_keeps_both_sides_of_renames_and_deleted_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)

            def git(*args):
                return subprocess.check_output(
                    ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                     "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", *args],
                    cwd=root, text=True,
                ).strip()

            git("init", "-q")
            (root / "old name.rs").write_text("fixture\n")
            (root / "deleted.rs").write_text("deleted fixture\n")
            git("add", ".")
            git("commit", "-qm", "fixture")
            base = git("rev-parse", "HEAD")
            (root / "old name.rs").rename(root / "new name.rs")
            (root / "deleted.rs").unlink()
            git("add", "-A")
            git("commit", "-qm", "rename fixture")
            head = git("rev-parse", "HEAD")
            for event in ("push", "pull_request"):
                self.assertEqual(plan.changed_paths(base, head, event, root),
                                 ["deleted.rs", "new name.rs", "old name.rs"])

    def test_pull_request_uses_merge_base(self):
        with patch.object(subprocess, "check_output", side_effect=["c" * 40 + "\n", b"backend/deleted.rs\0"]) as run:
            self.assertEqual(plan.changed_paths("a" * 40, "b" * 40, "pull_request"), ["backend/deleted.rs"])
            self.assertIn("c" * 40, run.call_args_list[1].args[0])


class CliTests(unittest.TestCase):
    def test_cargo_command_preserves_flags_and_warning_separator(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            cargo = directory / "cargo"
            cargo.write_text('''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys
Path(os.environ["RECORD"]).write_text(json.dumps({"args": sys.argv[1:], "cwd": os.getcwd()}))
sys.exit(int(os.environ.get("CARGO_EXIT", "0")))
''')
            cargo.chmod(0o755)
            env = {**os.environ, "PATH": str(directory) + os.pathsep + os.environ["PATH"],
                   "RECORD": str(directory / "record.json")}
            for operation, arguments in (
                ("clippy", ["--all-targets", "--all-features", "--locked", "--", "-D", "warnings"]),
                ("test", ["--lib", "--test", "main", "--locked"]),
            ):
                result = subprocess.run(
                    [sys.executable, str(ROOT / "release/ci_plan.py"), "cargo", "--packages",
                     '["gateway-api"]', operation, "--", *arguments],
                    env=env, text=True, capture_output=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                record = json.loads((directory / "record.json").read_text())
                self.assertEqual(record["args"], [operation, "--package", "gateway-api", *arguments])
                self.assertEqual(Path(record["cwd"]).resolve(), (ROOT / "backend").resolve())
            failed = subprocess.run(
                [sys.executable, str(ROOT / "release/ci_plan.py"), "cargo", "test", "--", "--locked"],
                env={**env, "CARGO_EXIT": "7"}, capture_output=True,
            )
            self.assertNotEqual(failed.returncode, 0)

    def test_manual_plan_emits_full_github_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "outputs"
            result = subprocess.run(
                [sys.executable, str(ROOT / "release/ci_plan.py"), "plan", "--head", "a" * 40,
                 "--event", "workflow_dispatch"], env={**os.environ, "GITHUB_OUTPUT": str(output)},
                text=True, capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(output.read_text().splitlines(), ["backend_packages=[]", "redis_acl=true"])


if __name__ == "__main__":
    unittest.main()
