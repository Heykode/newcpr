from datetime import datetime, timedelta, timezone
from contextlib import redirect_stderr, redirect_stdout
import io
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


class ReleaseEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.run, self.proof, _ = fixtures()
        self.run.update(id=30, run_attempt=1, head_sha=PARENT,
                        event="push", head_branch="main")
        self.job = {"name": "quality / backend", "status": "completed",
                    "conclusion": "success", "html_url": "https://example.invalid/jobs/1"}

    def evidence(self, main_jobs=(), source_jobs=(), changes="release/version.yaml\0",
                 parents=None, runs=None):
        def api(path):
            if path.endswith("ci.yml"):
                return {"id": 9}
            if "/workflows/9/runs?" in path:
                self.assertIn(f"event=push&branch=main&head_sha={PARENT}&per_page=1", path)
                return {"workflow_runs": [self.run] if runs is None else runs}
            if path.endswith("/runs/30"):
                return self.run
            if path.endswith("/runs/12"):
                return fixtures()[0]
            if path.endswith("/runs/30/jobs?filter=latest&per_page=100"):
                return {"jobs": main_jobs}
            self.assertTrue(path.endswith("/runs/12/jobs?filter=latest&per_page=100"), path)
            return {"jobs": source_jobs}

        with patch.object(images, "git", side_effect=[
                parents or f"{'e' * 40} {PARENT}", changes]), \
                patch.object(images, "api", side_effect=api) as remote, \
                patch.object(images, "resolve", return_value={"proof": self.proof}) as resolve:
            result = images.release_evidence(REPOSITORY)
            return result, remote, resolve

    def test_direct_full_main_validation_is_reused(self):
        result, _, resolve = self.evidence(main_jobs=[self.job])
        self.assertEqual(result, self.job["html_url"])
        resolve.assert_not_called()

    def test_merge_reuse_requires_the_proven_source_complete_backend(self):
        result, _, resolve = self.evidence(source_jobs=[self.job])
        self.assertEqual(result, self.job["html_url"])
        resolve.assert_called_once_with(REPOSITORY, PARENT)

    def test_partial_skipped_failed_or_missing_source_checks_are_not_reused(self):
        for jobs in ([], [{**self.job, "name": "quality / backend-selected"}],
                     [{**self.job, "status": "in_progress"}],
                     *[[{**self.job, "conclusion": state}]
                       for state in ("skipped", "failure", "cancelled")]):
            with self.subTest(jobs=jobs):
                self.assertIsNone(self.evidence(source_jobs=jobs)[0])

    def test_latest_main_failure_wrong_identity_or_expiry_cannot_use_old_pr(self):
        original = dict(self.run)
        for field, value in {
            "conclusion": "failure", "status": "in_progress", "head_sha": SOURCE,
            "head_repository": {"full_name": "other/repository"},
            "head_branch": "topic", "event": "pull_request", "workflow_id": 8,
            "path": ".github/workflows/build-cache.yml",
            "updated_at": (datetime.now(timezone.utc) - timedelta(days=8)).isoformat(),
        }.items():
            with self.subTest(field=field):
                self.run = {**original, field: value}
                result, _, resolve = self.evidence(source_jobs=[self.job])
                self.assertIsNone(result)
                resolve.assert_not_called()
        self.run = original
        result, _, resolve = self.evidence(runs=[])
        self.assertIsNone(result)
        resolve.assert_not_called()

    def test_only_nonempty_metadata_changes_on_single_parent_commits_are_eligible(self):
        for changes, parents in [
            ("", None), ("backend/source.rs\0", None),
            ("release/notes.md\0backend/source.rs\0", None),
            ("release/notes.md\0", SOURCE),
            ("release/notes.md\0", f"{SOURCE} {PARENT} {TREE}"),
        ]:
            with self.subTest(changes=changes, parents=parents):
                result, remote, resolve = self.evidence(changes=changes, parents=parents)
                self.assertIsNone(result)
                remote.assert_not_called()
                resolve.assert_not_called()

    def test_same_run_without_full_backend_is_not_a_reuse_chain(self):
        self.proof["run_id"] = self.run["id"]
        self.assertIsNone(self.evidence(source_jobs=[self.job])[0])

    def test_failed_job_rerun_retains_earlier_successful_backend(self):
        job = {**self.job, "run_attempt": 1}
        self.assertEqual(self.evidence(source_jobs=[job])[0], job["html_url"])

    def test_new_attempt_or_failure_during_evidence_resolution_is_rejected(self):
        run = fixtures()[0]
        for change in ({"run_attempt": 3}, {"status": "in_progress"}, {"conclusion": "failure"}):
            with self.subTest(change=change), patch.object(images, "api", return_value={**run, **change}), \
                    self.assertRaises(images.Unavailable):
                images.backend_job(REPOSITORY, 12, 2)

    def test_unavailable_proof_falls_back_to_full_checks_with_clean_output(self):
        for error in (images.Unavailable("expired proof"), OSError("API unavailable"),
                      ValueError("invalid proof"), KeyError("missing identity")):
            with self.subTest(error=type(error).__name__), \
                    patch.object(images, "release_evidence", side_effect=error), \
                    patch.dict(images.os.environ, {}, clear=True), \
                    redirect_stdout(io.StringIO()) as output, redirect_stderr(io.StringIO()):
                images.report_release_quality(REPOSITORY)
                self.assertEqual(output.getvalue(), "run_backend=true\n")

    def test_success_reports_reuse_without_requesting_release_or_deployment(self):
        with patch.object(images, "release_evidence", return_value=self.job["html_url"]), \
                patch.dict(images.os.environ, {}, clear=True), redirect_stdout(io.StringIO()) as output:
            images.report_release_quality(REPOSITORY)
            self.assertEqual(output.getvalue(), "run_backend=false\n")


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

    def test_timeout_retries_from_a_clean_archive(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "source.zip"
            with zipfile.ZipFile(source, "w") as bundle:
                bundle.writestr("manifest.json", "{}")
            artifact = {
                "id": 1, "expired": False, "size_in_bytes": source.stat().st_size,
                "digest": "sha256:" + images.sha256(source),
            }
            attempts = 0

            def run(args, stdout, **kwargs):
                nonlocal attempts
                attempts += 1
                self.assertEqual(kwargs["timeout"], images.ARTIFACT_DOWNLOAD_TIMEOUT)
                if attempts == 1:
                    stdout.write(b"incomplete")
                    raise subprocess.TimeoutExpired(args, kwargs["timeout"])
                stdout.write(source.read_bytes())
                return subprocess.CompletedProcess(args, 0)

            destination = Path(temporary) / "manifest.json"
            with patch.object(images.subprocess, "run", side_effect=run):
                images.artifact_file(REPOSITORY, artifact, "manifest.json", destination)
            self.assertEqual(attempts, 2)
            self.assertEqual(destination.read_text(), "{}")

    def test_retry_exhaustion_fails_before_extracting(self):
        artifact = {
            "id": 1, "expired": False, "size_in_bytes": 1,
            "digest": "sha256:" + "0" * 64,
        }
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "manifest.json"
            error = subprocess.TimeoutExpired(["gh"], images.ARTIFACT_DOWNLOAD_TIMEOUT)
            with patch.object(images.subprocess, "run", side_effect=error) as run, \
                    self.assertRaisesRegex(images.Unavailable, "repeated attempts"):
                images.artifact_file(REPOSITORY, artifact, "manifest.json", destination)
            self.assertEqual(run.call_count, images.ARTIFACT_DOWNLOAD_ATTEMPTS)
            self.assertFalse(destination.exists())

    def test_retry_keeps_artifact_digest_check(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "source.zip"
            with zipfile.ZipFile(source, "w") as bundle:
                bundle.writestr("manifest.json", "{}")
            artifact = {
                "id": 1, "expired": False, "size_in_bytes": source.stat().st_size,
                "digest": "sha256:" + "0" * 64,
            }
            attempts = 0

            def run(args, stdout, **kwargs):
                nonlocal attempts
                attempts += 1
                if attempts == 1:
                    raise subprocess.CalledProcessError(1, args)
                stdout.write(source.read_bytes())
                return subprocess.CompletedProcess(args, 0)

            destination = Path(temporary) / "manifest.json"
            with patch.object(images.subprocess, "run", side_effect=run), \
                    self.assertRaisesRegex(images.Unavailable, "digest mismatch"):
                images.artifact_file(REPOSITORY, artifact, "manifest.json", destination)
            self.assertEqual(attempts, 2)
            self.assertFalse(destination.exists())

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
