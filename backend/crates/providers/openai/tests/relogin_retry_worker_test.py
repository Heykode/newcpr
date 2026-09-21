"""Offline retry classification; reuses the existing synthetic login harness."""
import types
import unittest

from relogin_worker_test import login, worker


class FailureTests(unittest.TestCase):
    def test_only_explicit_terminal_codes_stop_login(self):
        for status, code, expected in [
            (403, "account_banned", "account_banned"),
            (401, "account_deactivated", "account_banned"),
            (400, "account_disabled", "account_banned"),
            (403, "account_suspended", "account_banned"),
            (403, "deactivated_workspace", "workspace_unavailable"),
            (401, "workspace_deactivated", "workspace_unavailable"),
            (403, "organization_disabled", "workspace_unavailable"),
            (401, "invalid_token", "upstream_rejected"),
            (403, "access_denied", "upstream_rejected"),
            (500, "account_banned", "upstream_rejected"),
            (429, "account_banned", "rate_limited"),
        ]:
            with self.subTest(status=status, code=code):
                instance = login()
                instance.session.request = lambda *args, **kwargs: types.SimpleNamespace(
                    content=b"{}", status_code=status,
                    json=lambda: {"error": {"code": code, "message": "test-only-private-diagnostic"}},
                )
                with self.assertRaisesRegex(worker.LoginError, f"^{expected}$"):
                    instance.fetch("GET", worker.AUTH + "/authorize")

    def test_text_html_and_wrong_password_are_not_proof_of_a_ban(self):
        for error in ["account_banned", {"message": "account_banned"}, None]:
            instance = login()
            instance.session.request = lambda *args, **kwargs: types.SimpleNamespace(
                content=b"{}", status_code=403, json=lambda: {"error": error},
            )
            with self.assertRaisesRegex(worker.LoginError, "^password_rejected$"):
                instance.fetch("POST", worker.AUTH + "/api/accounts/password/verify")


if __name__ == "__main__":
    unittest.main()
