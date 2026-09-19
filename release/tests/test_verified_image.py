from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import verified_image as images

SOURCE = "a" * 40
PARENT = "b" * 40
TREE = "c" * 40
MIGRATIONS = "d" * 40
REPOSITORY = "example/application"


def fixtures():
    now = datetime.now(timezone.utc).isoformat()
    run = {
        "id": 12, "run_attempt": 2, "head_sha": SOURCE, "workflow_id": 9,
        "head_repository": {"full_name": REPOSITORY}, "path": ".github/workflows/ci.yml",
        "event": "pull_request", "status": "completed", "conclusion": "success",
        "updated_at": now,
    }
    proof = {
        "schema": 1, "repository": REPOSITORY, "source_commit": SOURCE,
        "source_tree": TREE, "migrations_tree": MIGRATIONS, "platform": "linux/amd64",
        "run_id": 12, "run_attempt": 2, "created_at": now, "version": "1.2.3",
        "image_id": "sha256:" + "1" * 64, "image_tag": "codex-proxy-rs:ci-" + SOURCE,
        "archive_sha256": "2" * 64,
        "image_artifact": f"cpr-image-linux-amd64-{SOURCE}-2",
    }
    image = {
        "id": 21, "name": proof["image_artifact"], "expired": False,
        "digest": "sha256:" + "3" * 64, "size_in_bytes": 100,
    }
    return run, proof, image


class ProofTests(unittest.TestCase):
    def test_valid_proof(self):
        run, proof, image = fixtures()
        self.assertEqual(images.validate_proof(proof, run, REPOSITORY, SOURCE, TREE, [image]), image)

    def test_provenance_mismatches(self):
        run, original, image = fixtures()
        for key, value in {
            "schema": 2, "repository": "other/application", "source_commit": PARENT,
            "source_tree": PARENT, "platform": "linux/arm64", "run_id": 10, "run_attempt": 1,
            "image_tag": "mutable:latest", "image_artifact": "other", "image_id": "invalid",
            "archive_sha256": "invalid", "migrations_tree": "", "version": "bad",
            "created_at": "invalid",
        }.items():
            with self.subTest(key=key):
                proof = {**original, key: value}
                with self.assertRaises(images.Unavailable):
                    images.validate_proof(proof, run, REPOSITORY, SOURCE, TREE, [image])

    def test_missing_expired_duplicate_and_digestless_images(self):
        run, proof, image = fixtures()
        for artifacts in ([], [image, image], [{**image, "expired": True}], [{**image, "digest": ""}]):
            with self.subTest(artifacts=artifacts), self.assertRaises(images.Unavailable):
                images.validate_proof(proof, run, REPOSITORY, SOURCE, TREE, artifacts)

    def test_freshness(self):
        now = datetime.now(timezone.utc)
        self.assertTrue(images.fresh((now - timedelta(days=1)).isoformat(), now))
        for value in (None, "", "bad", "2020-01-01T00:00:00",
                      (now + timedelta(seconds=1)).isoformat(),
                      (now - timedelta(days=8)).isoformat()):
            with self.subTest(value=value):
                self.assertFalse(images.fresh(value, now))

    def test_run_identity(self):
        run, _, _ = fixtures()
        self.assertTrue(images.trusted_run(run, REPOSITORY, SOURCE, 9))
        for key, value in {
            "head_sha": PARENT, "workflow_id": 8, "head_repository": {"full_name": "other/app"},
            "path": ".github/workflows/release.yml", "event": "pull_request_target",
            "status": "in_progress", "conclusion": "failure", "updated_at": "",
        }.items():
            with self.subTest(key=key):
                self.assertFalse(images.trusted_run({**run, key: value}, REPOSITORY, SOURCE, 9))


class CandidateTests(unittest.TestCase):
    def test_identical_tree_parent_only(self):
        def git(*args):
            if args[0] == "show":
                return PARENT + " " + "e" * 40
            return TREE if args[1][:40] in {SOURCE, PARENT} else "f" * 40
        with patch.object(images, "git", side_effect=git):
            self.assertEqual(images.candidates(SOURCE), [SOURCE, PARENT])

    def test_non_merge_does_not_reuse_ancestor(self):
        with patch.object(images, "git", side_effect=[TREE, PARENT, TREE]):
            self.assertEqual(images.candidates(SOURCE), [SOURCE])

    def test_reject_short_sha(self):
        with self.assertRaises(images.Unavailable):
            images.candidates("abcdef")


class ResolveTests(unittest.TestCase):
    def resolve_with(self, runs, proof=None):
        _, default, image = fixtures()
        proof = default if proof is None else proof
        artifact = {"name": f"cpr-proof-{SOURCE}-2", "expired": False}

        def api(path):
            if path.endswith("ci.yml"):
                return {"id": 9}
            if "/artifacts?" in path:
                return {"artifacts": [artifact, image]}
            return {"workflow_runs": runs}

        def download(repository, artifact, member, path):
            Path(path).write_text(json.dumps(proof))

        with patch.object(images, "api", side_effect=api), \
                patch.object(images, "candidates", return_value=[SOURCE]), \
                patch.object(images, "git", side_effect=lambda *args: MIGRATIONS if ":" in args[1] else TREE), \
                patch.object(images, "artifact_file", side_effect=download):
            return images.resolve(REPOSITORY, SOURCE)

    def test_resolve_valid(self):
        run, _, _ = fixtures()
        self.assertEqual(self.resolve_with([run])["target_commit"], SOURCE)

    def test_newest_failure_cannot_fall_back_to_old_success(self):
        run, _, _ = fixtures()
        for state in ("failure", "cancelled"):
            with self.subTest(state=state), self.assertRaises(images.Unavailable):
                self.resolve_with([run, {**run, "id": 13, "conclusion": state}])

    def test_migration_proof_must_match_git(self):
        run, proof, _ = fixtures()
        proof["migrations_tree"] = PARENT
        with self.assertRaises(images.Unavailable):
            self.resolve_with([run], proof)

    def test_security_requires_all_jobs(self):
        run, _, _ = fixtures()
        run["path"] = ".github/workflows/security-scan.yml"
        for names, success in [
            (images.SECURITY_JOBS, True), ({"Privacy Guard"}, False),
            ({"backend-security", "frontend-security"}, False),
        ]:
            with self.subTest(names=names), patch.object(images, "api", side_effect=[
                {"id": 9}, {"workflow_runs": [run]},
                {"jobs": [{"name": name, "conclusion": "success"} for name in names]},
            ]):
                if success:
                    images.require_security(REPOSITORY, SOURCE)
                else:
                    with self.assertRaises(images.Unavailable):
                        images.require_security(REPOSITORY, SOURCE)


class ArtifactTests(unittest.TestCase):
    def download(self, entries, wrong_digest=False, expired=False):
        with tempfile.TemporaryDirectory() as temporary:
            archive = Path(temporary) / "source.zip"
            with zipfile.ZipFile(archive, "w") as bundle:
                for name, value in entries:
                    bundle.writestr(name, value)
            artifact = {
                "id": 1, "expired": expired, "size_in_bytes": archive.stat().st_size,
                "digest": "sha256:" + ("0" * 64 if wrong_digest else images.sha256(archive)),
            }

            def run(args, stdout, **kwargs):
                stdout.write(archive.read_bytes())
                return subprocess.CompletedProcess(args, 0)

            destination = Path(temporary) / "manifest.json"
            with patch.object(images.subprocess, "run", side_effect=run):
                images.artifact_file(REPOSITORY, artifact, "manifest.json", destination)
            return destination.read_text()

    def test_exact_member(self):
        self.assertEqual(self.download([("manifest.json", "{}")]), "{}")

    def test_reject_digest_mismatch(self):
        with self.assertRaises(images.Unavailable):
            self.download([("manifest.json", "{}")], wrong_digest=True)

    def test_reject_expired(self):
        with self.assertRaises(images.Unavailable):
            self.download([("manifest.json", "{}")], expired=True)

    def test_reject_unexpected_members_and_oversize_manifest(self):
        for entries in [
            [("../manifest.json", "{}")], [("manifest.json", "{}"), ("extra", "")],
            [("manifest.json", "x" * (1024**2 + 1))],
        ]:
            with self.subTest(names=[item[0] for item in entries]), self.assertRaises(images.Unavailable):
                self.download(entries)


if __name__ == "__main__":
    unittest.main()
