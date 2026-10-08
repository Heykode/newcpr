import contextlib
import io
import json
from pathlib import Path
import shlex
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import deploy
import verified_image as images
from test_verified_image import fixtures, SOURCE, MIGRATIONS, REPOSITORY


def profile():
    return {
        "repository": REPOSITORY, "ssh_host": "example-alias",
        "compose_file": "/srv/example/compose.yaml", "service": "app",
        "container": "example-app-1", "health_url": "http://127.0.0.1:8080/healthz",
        "protected_containers": ["example-db-1"], "config_files": ["/srv/example/config.yaml"],
    }


class DeployTests(unittest.TestCase):
    def test_profile(self):
        self.assertEqual(deploy.validate_profile(profile()), profile())

    def test_invalid_profiles(self):
        for field, value in {
            "repository": "../repo", "ssh_host": "-oProxyCommand=invalid",
            "compose_file": "relative.yaml", "service": "--invalid", "container": None,
            "health_url": "file:///etc/hosts", "health_status": 500,
            "protected_containers": "example-db-1", "config_files": ["relative.yaml"],
        }.items():
            with self.subTest(field=field), self.assertRaises(images.Unavailable):
                deploy.validate_profile({**profile(), field: value})

    def test_app_cannot_be_protected(self):
        item = profile()
        item["protected_containers"].append(item["container"])
        with self.assertRaises(images.Unavailable):
            deploy.validate_profile(item)

    def test_egress_profile_validation(self):
        checks = [{"name": "ipv4", "url": "https://probe.example.test/", "family": "ipv4"},
                  {"name": "proxy", "url": "https://probe.example.test/", "proxy_id": "proxy_example"}]
        self.assertEqual(deploy.validate_profile({**profile(), "egress_checks": checks})["egress_checks"], checks)
        with self.assertRaises(images.Unavailable):
            deploy.validate_profile({**profile(), "egress_checks": [{"name": "invalid"}]})
        self.assertIn("deploy/egress_check.py", deploy.DEPLOY_FILES)

    def test_upgrade_boundaries(self):
        _, proof, _ = fixtures()
        labels = {"org.opencontainers.image.revision": SOURCE, "org.opencontainers.image.version": "1.1.0"}
        with patch.object(images, "git", return_value=MIGRATIONS):
            deploy.validate_upgrade(labels, proof)

    def test_reviewed_upgrade_is_bound_to_exact_versions_and_trees(self):
        import json
        _, proof, _ = fixtures()
        plan = {
            "from_version": "1.1.0", "to_version": proof["version"],
            "from_migrations_tree": MIGRATIONS, "to_migrations_tree": MIGRATIONS,
            "added": ["0028_example.sql"], "recovery": "manual",
        }
        labels = {"org.opencontainers.image.revision": SOURCE, "org.opencontainers.image.version": "1.1.0"}
        def git(*args):
            if args[0] == "show":
                return json.dumps(plan)
            if args[0] == "rev-parse":
                return MIGRATIONS
            if args[0] == "diff":
                return "A\tbackend/migrations/0028_example.sql\nM\tbackend/migrations/.frozen-sha256"
            return "backend/migrations/0001_example.sql"
        with patch.object(images, "git", side_effect=git), \
                patch.object(deploy.subprocess, "check_output", return_value=b"select 1;\n"):
            result = deploy.reviewed_upgrade("v1.1.1", labels, proof, SOURCE)
            self.assertEqual(result["before"], result["after"])
            self.assertEqual(len(result["before"]["1"]), 96)
            with self.assertRaises(images.Unavailable):
                deploy.reviewed_upgrade("../invalid", labels, proof, SOURCE)
            with self.assertRaises(images.Unavailable):
                deploy.reviewed_upgrade("v1.1.1", {}, proof, SOURCE)
            with self.assertRaises(images.Unavailable):
                deploy.reviewed_upgrade("v1.1.1", {**labels, "org.opencontainers.image.version": "9.0.0"}, proof, SOURCE)

    def test_reviewed_upgrade_rejects_modified_migration(self):
        import json
        _, proof, _ = fixtures()
        plan = {
            "from_version": "1.1.0", "to_version": proof["version"],
            "from_migrations_tree": MIGRATIONS, "to_migrations_tree": MIGRATIONS,
            "added": ["0028_example.sql"], "recovery": "manual",
        }
        labels = {"org.opencontainers.image.revision": SOURCE, "org.opencontainers.image.version": "1.1.0"}
        with patch.object(images, "git", side_effect=[
            json.dumps(plan), json.dumps(plan), MIGRATIONS,
            "M\tbackend/migrations/0001_example.sql",
        ]), self.assertRaises(images.Unavailable):
            deploy.reviewed_upgrade("v1.1.1", labels, proof, SOURCE)
            with self.assertRaises(images.Unavailable):
                deploy.validate_upgrade({}, proof)
            with self.assertRaises(images.Unavailable):
                deploy.validate_upgrade({**labels, "org.opencontainers.image.version": "2.0.0"}, proof)
        with patch.object(images, "git", return_value="b" * 40), self.assertRaises(images.Unavailable):
            deploy.validate_upgrade(labels, proof)

    def test_quality_group_upgrade_keeps_same_version_but_requires_exact_schema(self):
        plan = json.loads((deploy.ROOT / "deploy/upgrades/v3.19.1.json").read_text())
        self.assertEqual(plan["from_version"], "3.19.1")
        self.assertEqual(plan["to_version"], "3.19.1")
        self.assertNotEqual(plan["from_migrations_tree"], plan["to_migrations_tree"])
        self.assertEqual(plan["added"], [
            "0054_request_capture_lookup.sql", "0055_quality_group_rules.sql",
        ])
        self.assertEqual(plan["recovery"], "manual")
        target = "e" * 40
        labels = {
            "org.opencontainers.image.revision": SOURCE,
            "org.opencontainers.image.version": plan["from_version"],
        }
        _, proof, _ = fixtures()
        proof = {**proof, "version": plan["to_version"],
                 "migrations_tree": plan["to_migrations_tree"]}
        source_tree = plan["from_migrations_tree"]

        def git(*args):
            if args[0] == "show":
                return json.dumps(plan)
            if args[0] == "rev-parse":
                return source_tree
            if args[0] == "diff":
                return "\n".join([
                    "M\tbackend/migrations/.frozen-sha256",
                    *("A\tbackend/migrations/" + name for name in plan["added"]),
                ])
            paths = ["backend/migrations/0001_example.sql"]
            if args[3] == target:
                paths.extend("backend/migrations/" + name for name in plan["added"])
            return "\n".join(paths)

        with patch.object(images, "git", side_effect=git), \
                patch.object(deploy.subprocess, "check_output", return_value=b"select 1;\n"):
            result = deploy.reviewed_upgrade("v3.19.1", labels, proof, target)
            self.assertEqual(set(result["before"]), {"1"})
            self.assertEqual(set(result["after"]), {"1", "54", "55"})
            self.assertEqual(result["before"]["1"], result["after"]["1"])
            with self.assertRaises(images.Unavailable):
                deploy.reviewed_upgrade("v3.19.1", {
                    **labels, "org.opencontainers.image.version": "3.19.0",
                }, proof, target)
            with self.assertRaises(images.Unavailable):
                deploy.reviewed_upgrade("v3.19.1", labels, {
                    **proof, "migrations_tree": source_tree,
                }, target)
            source_tree = "f" * 40
            with self.assertRaises(images.Unavailable):
                deploy.reviewed_upgrade("v3.19.1", labels, proof, target)

    def test_v3201_upgrade_matches_existing_v3191_schema_and_added_migrations(self):
        plan = json.loads((deploy.ROOT / "deploy/upgrades/v3.20.1.json").read_text())
        self.assertEqual(plan["from_version"], "3.19.1")
        self.assertEqual(plan["to_version"], "3.20.1")
        self.assertEqual(plan["from_migrations_tree"], "906f084734203e93db61f2d4dca7a908f0075d34")
        self.assertEqual(plan["to_migrations_tree"], "dffc11221dd5d18eef9f12d6adf2307d60379a7d")
        self.assertEqual(plan["added"], [
            "0056_log_cleanup.sql", "0057_account_purchase_costs.sql",
            "0058_excel_paused_recovery.sql", "0060_account_reset_credits.sql",
            "0061_guardian_reserved_concurrency.sql", "0062_account_warmup.sql",
        ])
        self.assertEqual(plan["recovery"], "manual")

    def test_v3220_upgrade_includes_undeployed_v3210_and_auto_reset_migrations(self):
        plan = json.loads((deploy.ROOT / "deploy/upgrades/v3.22.0.json").read_text())
        self.assertEqual(plan["from_version"], "3.20.1")
        self.assertEqual(plan["to_version"], "3.22.0")
        self.assertEqual(plan["from_migrations_tree"], "dffc11221dd5d18eef9f12d6adf2307d60379a7d")
        self.assertEqual(plan["to_migrations_tree"], "9f2a609cc8ee00680e3eacddcc1af5d6b5dbe57f")
        self.assertEqual(plan["added"], [
            "0063_refresh_margin_default.sql", "0064_group_fast_mode.sql",
            "0065_openai_session_binding_ttl.sql", "0066_openai_account_affinity.sql",
            "0067_account_auto_reset.sql",
        ])
        self.assertEqual(plan["recovery"], "manual")

    def test_reviewed_documentation_requires_exact_declaration_and_diff(self):
        _, proof, _ = fixtures()
        base = {
            "from_version": "1.1.0", "to_version": proof["version"],
            "from_migrations_tree": MIGRATIONS, "to_migrations_tree": MIGRATIONS,
            "added": ["0028_example.sql"], "recovery": "manual",
        }
        labels = {"org.opencontainers.image.revision": SOURCE,
                  "org.opencontainers.image.version": "1.1.0"}
        additions = "A\tbackend/migrations/0028_example.sql\nM\tbackend/migrations/.frozen-sha256"
        readme = "\nM\tbackend/migrations/README.md"
        cases = [
            ({}, additions, True),
            ({}, additions + readme, False),
            ({"modified_documentation": []}, additions, True),
            ({"modified_documentation": ["README.md"]}, additions + readme, True),
            ({"modified_documentation": ["README.md"]}, additions, False),
            ({"modified_documentation": ["README.md"]}, additions + readme
             + "\nM\tbackend/migrations/0001_example.sql", False),
            ({"modified_documentation": ["README.md"]}, additions
             + "\nA\tbackend/migrations/README.md", False),
            ({"modified_documentation": ["0001_example.sql"]}, additions
             + "\nM\tbackend/migrations/0001_example.sql", False),
        ]
        cases.extend(({"modified_documentation": value}, additions, False) for value in (
            None, "README.md", {}, ["../README.md"], ["other.md"],
            ["README.md", "README.md"], ["README.md", "0001_example.sql"],
        ))
        for fields, changes, accepted in cases:
            with self.subTest(fields=fields, changes=changes):
                def git(*args):
                    if args[0] == "show":
                        return json.dumps({**base, **fields})
                    if args[0] == "rev-parse":
                        return MIGRATIONS
                    if args[0] == "diff":
                        return changes
                    return "backend/migrations/0001_example.sql\nbackend/migrations/README.md"

                with patch.object(images, "git", side_effect=git), \
                        patch.object(deploy.subprocess, "check_output", return_value=b"select 1;\n"):
                    if accepted:
                        result = deploy.reviewed_upgrade("v1.1.1", labels, proof, SOURCE)
                        self.assertEqual(set(result["before"]), {"1"})
                        self.assertEqual(result["before"], result["after"])
                    else:
                        with self.assertRaises(images.Unavailable):
                            deploy.reviewed_upgrade("v1.1.1", labels, proof, SOURCE)

    def test_v3230_upgrade_preserves_existing_affinity_and_requires_manual_recovery(self):
        plan = json.loads((deploy.ROOT / "deploy/upgrades/v3.23.0.json").read_text())
        self.assertEqual(plan["from_version"], "3.22.1")
        self.assertEqual(plan["to_version"], "3.23.0")
        self.assertEqual(plan["from_migrations_tree"], "9f2a609cc8ee00680e3eacddcc1af5d6b5dbe57f")
        self.assertEqual(plan["to_migrations_tree"], "a36f8520df448be5c84d0419ac37d1052661b5a2")
        self.assertEqual(plan["added"], ["0068_preferred_account_affinity.sql"])
        self.assertEqual(plan["modified_documentation"], ["README.md"])
        self.assertEqual(plan["recovery"], "manual")

    def test_latest_main_ci_required(self):
        run, _, _ = fixtures()
        run["event"] = "push"
        for runs, success in [([run], True), ([{**run, "event": "pull_request"}], False),
                              ([run, {**run, "id": 99, "conclusion": "failure"}], False)]:
            with self.subTest(runs=runs), patch.object(images, "api", side_effect=[
                {"id": 9}, {"workflow_runs": runs},
            ]):
                if success:
                    deploy.require_target_ci(REPOSITORY, SOURCE)
                else:
                    with self.assertRaises(images.Unavailable):
                        deploy.require_target_ci(REPOSITORY, SOURCE)

    def test_stale_scripts_fail_before_remote_access(self):
        with patch.object(images, "git", side_effect=["", "old", "new"]), \
                patch.object(deploy, "ssh") as ssh, self.assertRaises(images.Unavailable):
            deploy.run(profile(), SOURCE, False)
        ssh.assert_not_called()

    def test_plan_does_not_transfer_or_apply(self):
        _, proof, artifact = fixtures()
        with contextlib.redirect_stdout(io.StringIO()), \
                patch.object(images, "git", return_value="same"), \
                patch.object(images, "command", return_value=REPOSITORY), \
                patch.object(images, "require_security") as security, \
                patch.object(images, "resolve", return_value={"proof": proof, "artifact": artifact}), \
                patch.object(deploy, "current_image", return_value=("old", {})), \
                patch.object(deploy, "validate_upgrade"), \
                patch.object(deploy, "require_target_ci"), \
                patch.object(images, "artifact_file") as download, \
                patch.object(deploy, "ssh") as ssh, \
                patch.object(deploy.subprocess, "run") as command:
            deploy.run(profile(), SOURCE, False)
        download.assert_not_called()
        ssh.assert_not_called()
        security.assert_called_once_with(REPOSITORY, SOURCE)
        self.assertEqual(command.call_count, 1)
        self.assertEqual(command.call_args.args[0][0], "git")

    def test_invalid_modes_fail_before_fetch_or_remote_access(self):
        for options in (
            {"apply": True, "prepare": True},
            {"apply": False, "staged": "/var/tmp/cpr-deploy.example"},
            {"apply": False, "prepare": True, "staged": "/var/tmp/cpr-deploy.example"},
            {"apply": True, "staged": "/var/tmp/cpr-deploy.example/../other"},
            {"apply": True, "staged": ""},
        ):
            with self.subTest(options=options), patch.object(images, "git") as git, \
                    patch.object(deploy, "ssh") as ssh, self.assertRaises(images.Unavailable):
                deploy.run(profile(), SOURCE, **options)
            git.assert_not_called()
            ssh.assert_not_called()

    def run_mutation(self, *, prepare=False, staged=None, change_request=None,
                     archive_digest=None, server_status=0):
        _, proof, artifact = fixtures()
        selected = {"target_commit": SOURCE, "proof": proof, "artifact": artifact}
        request = {"profile": profile(), "selected": selected, "expected_old_image": "old"}
        if change_request:
            change_request(request)
        remote = "/var/tmp/cpr-deploy.example"
        output = io.StringIO()
        commands = []
        def command(args, **kwargs):
            commands.append(args)
            return subprocess.CompletedProcess(args, server_status if args[0] == "ssh" else 0)
        with contextlib.redirect_stdout(output), \
                patch.object(images, "git", return_value="same"), \
                patch.object(images, "command", return_value=REPOSITORY), \
                patch.object(images, "require_security") as security, \
                patch.object(images, "resolve", return_value=selected), \
                patch.object(deploy, "current_image", return_value=("old", {})), \
                patch.object(deploy, "validate_upgrade"), \
                patch.object(deploy, "require_target_ci") as ci, \
                patch.object(images, "artifact_file") as download, \
                patch.object(images, "sha256", return_value=archive_digest or proof["archive_sha256"]), \
                patch.object(deploy, "ssh", return_value=json.dumps(request) if staged else remote) as ssh, \
                patch.object(deploy.subprocess, "run", side_effect=command):
            deploy.run(profile(), SOURCE, not prepare, prepare=prepare, staged=staged)
        return commands, output.getvalue(), download, ssh, ci, security

    def test_prepare_invokes_only_server_prepare(self):
        commands, output, download, _, ci, security = self.run_mutation(prepare=True)
        self.assertEqual([args[0] for args in commands], ["git", "scp", "ssh"])
        args = shlex.split(commands[-1][-1])
        self.assertIn("--prepare", args)
        self.assertNotIn("--apply", args)
        self.assertIn("prepared_not_deployed", output)
        self.assertIn("/var/tmp/cpr-deploy.example", output)
        download.assert_called_once()
        ci.assert_called_once_with(REPOSITORY, SOURCE)
        security.assert_called_once_with(REPOSITORY, SOURCE)

    def test_prepared_apply_reuses_archive_but_rechecks_evidence(self):
        commands, output, download, ssh, ci, security = self.run_mutation(
            staged="/var/tmp/cpr-deploy.example")
        download.assert_not_called()
        ssh.assert_called_once_with("example-alias", "cat", "/var/tmp/cpr-deploy.example/request.json")
        self.assertNotIn("image.tar.gz", " ".join(commands[1]))
        self.assertIn("--apply", shlex.split(commands[-1][-1]))
        self.assertNotIn("prepared_not_deployed", output)
        ci.assert_called_once_with(REPOSITORY, SOURCE)
        security.assert_called_once_with(REPOSITORY, SOURCE)

    def test_prepared_request_mismatch_refuses_transfer_and_switch(self):
        for field in ("profile", "selected", "expected_old_image", "migration_upgrade"):
            with self.subTest(field=field), self.assertRaisesRegex(images.Unavailable, "Prepared request"):
                self.run_mutation(staged="/var/tmp/cpr-deploy.example",
                                  change_request=lambda request: request.update({field: "changed"}))

    def test_prepare_rejects_bad_download_or_server_failure(self):
        with self.assertRaisesRegex(images.Unavailable, "checksum"):
            self.run_mutation(prepare=True, archive_digest="invalid")
        with self.assertRaisesRegex(images.Unavailable, "failed"):
            self.run_mutation(prepare=True, server_status=1)

    def test_apply_without_staging_preserves_existing_workflow(self):
        commands, output, download, _, _, _ = self.run_mutation()
        download.assert_called_once()
        self.assertIn("image.tar.gz", " ".join(commands[1]))
        self.assertIn("--apply", shlex.split(commands[-1][-1]))
        self.assertNotIn("prepared_not_deployed", output)

    def test_cli_rejects_conflicting_modes(self):
        result = subprocess.run([
            sys.executable, str(Path(deploy.__file__)), "--profile", "/nonexistent",
            "--commit", SOURCE, "--prepare", "--apply",
        ], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn("not allowed", result.stderr)


if __name__ == "__main__":
    unittest.main()
