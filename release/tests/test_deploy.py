import contextlib
import io
from pathlib import Path
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


if __name__ == "__main__":
    unittest.main()
