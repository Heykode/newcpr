"""Synthetic Excel OAuth fixtures. No upstream requests or real login material."""

import io
import json
import time
import types
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlparse

from relogin_worker_test import login, response, token, worker


class ExcelLoginTests(unittest.TestCase):
    def fixture(self, client=None, workspace="team-id", allowed=True):
        instance = login()
        instance.request["credential_target"] = "excel"
        instance.selected = {"id": "team-id", "plan": "business"}
        access = token({
            "client_id": client or worker.EXCEL_CLIENT, "exp": int(time.time()) + 3600,
            "https://api.openai.com/auth": {"chatgpt_account_id": workspace,
                "chatgpt_user_id": "fixture-user", "chatgpt_plan_type": "team"},
            "https://api.openai.com/profile": {"email": "test@example.invalid"},
        })
        calls = []

        def flow(url, oauth=False, state=None):
            parsed = urlparse(url)
            params = parse_qs(parsed.query)
            self.assertEqual(parsed.path, "/api/accounts/authorize")
            self.assertEqual(params["client_id"], [worker.EXCEL_CLIENT])
            self.assertEqual(params["redirect_uri"], [worker.EXCEL_CALLBACK])
            self.assertEqual(params["scope"], ["openid offline_access email profile organization.read"])
            self.assertEqual(params["platform"], ["PC"])
            self.assertNotIn("codex_cli_simplified_flow", params)
            self.assertTrue(oauth)
            self.assertEqual(params["state"], [state])
            return "fixture-code"

        def fetch(method, url, **kwargs):
            calls.append((method, url, kwargs))
            self.assertEqual(url, worker.AUTH + "/oauth/token")
            self.assertEqual(kwargs["data"]["client_id"], worker.EXCEL_CLIENT)
            self.assertEqual(kwargs["data"]["redirect_uri"], worker.EXCEL_CALLBACK)
            return response(url, {"access_token": access, "refresh_token": "fixture-refresh",
                                  "token_type": "Bearer"})

        def request(method, url, **kwargs):
            calls.append((method, url, kwargs))
            self.assertEqual(url, worker.EXCEL_ACCESS)
            headers = kwargs["headers"]
            self.assertEqual(headers["User-Agent"], "Mozilla/5.0")
            self.assertEqual(headers["X-Openai-Account-User-Id"], "fixture-user")
            self.assertEqual(headers["X-Basispoints-Auth-Mode"], "chatgpt")
            self.assertEqual(headers["X-Openai-Internal-Basispoints-Client-Product"], "basispoints-excel-plugin")
            self.assertFalse(kwargs["allow_redirects"])
            body = {"allowed": allowed, "model_catalog": {"models": [{"id": "gpt-example"}]}}
            return types.SimpleNamespace(content=b"{}", status_code=200, json=lambda: body)

        instance.flow = flow
        instance.fetch = fetch
        instance.session.request = request
        return instance, calls

    def full_fixture(self, choices=None, locked_workspace="team-id", allowed=True):
        instance, calls = self.fixture(allowed=allowed)
        instance.selected = None
        if locked_workspace:
            instance.request["workspace_id"] = locked_workspace
        instance.flow = types.MethodType(worker.Login.flow, instance)
        token_fetch = instance.fetch
        authorization = {}
        pages = {
            "/api/accounts/authorize/continue": ("login_password", {}),
            "/api/accounts/password/verify": ("mfa_challenge", {
                "factors": [{"factor_type": "totp", "id": "fixture-factor"}],
            }),
            "/api/accounts/mfa/verify": ("workspace_select", {
                "workspaces": choices if choices is not None else [
                    {"id": "free-id", "plan_type": "free"},
                    {"id": "team-id", "plan_type": "team"},
                ],
            }),
            "/api/accounts/workspace/select": ("consent", {}),
        }

        def fetch(method, url, **kwargs):
            parsed = urlparse(url)
            self.assertEqual(parsed.netloc, "auth.openai.com")
            if parsed.path == "/oauth/token":
                return token_fetch(method, url, **kwargs)
            calls.append((method, url, kwargs))
            if parsed.path == "/api/accounts/authorize":
                self.assertEqual(method, "GET")
                params = parse_qs(parsed.query)
                self.assertEqual(params["client_id"], [worker.EXCEL_CLIENT])
                self.assertEqual(params["audience"], ["https://api.openai.com/v1"])
                self.assertEqual(params["code_challenge_method"], ["S256"])
                authorization["state"] = params["state"][0]
                return response(url, {"page": {"type": "login_identifier"}})
            self.assertEqual(method, "POST")
            if parsed.path == "/api/accounts/consent/grant":
                return response(url, location=worker.EXCEL_CALLBACK
                                + "?code=fixture-code&state=" + authorization["state"])
            kind, payload = pages[parsed.path]
            return response(url, {"page": {"type": kind, "payload": payload}})

        instance.fetch = fetch
        return instance, calls, pages

    def test_excel_uses_independent_authorization_and_access_validation(self):
        instance, calls = self.fixture()
        result = instance.run_excel_oauth()
        self.assertTrue(result["ok"])
        self.assertEqual(result["document"]["type"], "cpa-bps")
        self.assertEqual(result["document"]["chatgpt_account_id"], "team-id")
        self.assertEqual(result["document"]["user_id"], "fixture-user")
        self.assertEqual(len(calls), 2)
        self.assertFalse(any("wham" in url for _, url, _ in calls))

    def test_excel_run_starts_at_bps_oauth_without_a_web_session(self):
        instance, calls = self.fixture()
        instance.selected = None
        instance.request["workspace_id"] = "team-id"
        result = instance.run()
        self.assertTrue(result["ok"])
        self.assertEqual(result["document"]["chatgpt_account_id"], "team-id")
        self.assertEqual([url for _, url, _ in calls],
                         [worker.AUTH + "/oauth/token", worker.EXCEL_ACCESS])

    def test_new_excel_account_accepts_the_authorized_token_workspace(self):
        instance, calls = self.fixture()
        instance.selected = None
        result = instance.run()
        self.assertTrue(result["ok"])
        self.assertEqual(result["document"]["chatgpt_account_id"], "team-id")
        self.assertFalse(any(url.startswith(worker.WEB) for _, url, _ in calls))

    def test_codex_keeps_the_existing_web_login_entry(self):
        instance = login()
        calls = []

        def fetch(method, url, **kwargs):
            calls.append((method, url))
            raise worker.LoginError("upstream_rejected")

        instance.fetch = fetch
        instance.run_excel_oauth = lambda: self.fail("Codex entered Excel OAuth")
        with self.assertRaisesRegex(worker.LoginError, "^upstream_rejected$"):
            instance.run()
        self.assertEqual(calls, [("GET", worker.WEB + "/api/auth/csrf")])

    def test_excel_full_run_completes_password_totp_workspace_and_consent(self):
        instance, calls, _ = self.full_fixture()
        with patch.object(worker.time, "time", return_value=60):
            result = instance.run()
        self.assertTrue(result["ok"])
        paths = [urlparse(url).path for _, url, _ in calls]
        self.assertEqual(paths, [
            "/api/accounts/authorize", "/api/accounts/authorize/continue",
            "/api/accounts/password/verify", "/api/accounts/mfa/verify",
            "/api/accounts/workspace/select", "/api/accounts/consent/grant",
            "/oauth/token", "/basispoints/api/responses/access",
        ])
        self.assertEqual(calls[3][2]["json"],
                         {"id": "fixture-factor", "type": "totp", "code": "123456"})
        self.assertEqual(calls[4][2]["json"], {"workspace_id": "team-id"})
        self.assertFalse(any(url.startswith(worker.WEB) for _, url, _ in calls))
        self.assertFalse(any(url.startswith(worker.EXCEL_CALLBACK) for _, url, _ in calls))

    def test_new_excel_account_selects_one_oauth_workspace_without_web_memberships(self):
        instance, calls, _ = self.full_fixture(choices=[{"id": "team-id"}], locked_workspace=None)
        with patch.object(worker.time, "time", return_value=60):
            result = instance.run()
        self.assertTrue(result["ok"])
        self.assertEqual(calls[4][2]["json"], {"workspace_id": "team-id"})

    def test_new_excel_account_preserves_known_plan_selection_policy(self):
        instance, calls, _ = self.full_fixture(locked_workspace=None)
        with patch.object(worker.time, "time", return_value=60):
            result = instance.run()
        self.assertTrue(result["ok"])
        self.assertEqual(calls[4][2]["json"], {"workspace_id": "team-id"})

    def test_excel_workspace_ambiguity_is_not_guessed(self):
        instance, calls, _ = self.full_fixture(choices=[
            {"id": "team-id", "name": "First", "plan_type": "team"},
            {"id": "other-team", "name": "Second", "plan_type": "business"},
        ], locked_workspace=None)
        with patch.object(worker.time, "time", return_value=60):
            with self.assertRaises(worker.WorkspaceSelectionRequired) as raised:
                instance.run()
        self.assertEqual({choice["id"] for choice in raised.exception.choices}, {"team-id", "other-team"})
        self.assertEqual(instance.stage, "workspace")
        self.assertEqual(len(calls), 4)

    def test_excel_unknown_workspace_plans_and_missing_locked_workspace_fail_closed(self):
        cases = [
            ([{"id": "team-id"}, {"id": "other-team"}], None, "workspace_plan_unknown"),
            ([{"id": "other-team", "plan_type": "team"}], "team-id", "workspace_missing"),
            ([], "team-id", "workspace_missing"),
            ([{"id": "invalid workspace"}], None, "workspace_unknown"),
        ]
        for choices, locked, expected in cases:
            with self.subTest(expected=expected, choices=choices):
                instance, calls, _ = self.full_fixture(choices=choices, locked_workspace=locked)
                with patch.object(worker.time, "time", return_value=60):
                    with self.assertRaisesRegex(worker.LoginError, "^" + expected + "$"):
                        instance.run()
                self.assertEqual(instance.stage, "workspace")
                self.assertEqual(len(calls), 4)

    def test_excel_interactive_pages_stop_before_token_exchange(self):
        for kind in ("captcha", "email_otp_verification", "add_phone", "unknown_page"):
            with self.subTest(kind=kind):
                instance, calls, pages = self.full_fixture()
                pages["/api/accounts/password/verify"] = (kind, {})
                with self.assertRaisesRegex(worker.LoginError, "^interaction_required$"):
                    instance.run()
                self.assertEqual(instance.stage, "password")
                self.assertEqual(len(calls), 3)

    def test_excel_missing_totp_factor_and_login_loop_report_their_stage(self):
        cases = [
            ("/api/accounts/password/verify", "mfa_challenge", "mfa_factor_missing", "mfa"),
            ("/api/accounts/authorize/continue", "login_identifier", "login_loop", "authorize"),
        ]
        for endpoint, kind, expected, stage in cases:
            with self.subTest(expected=expected):
                instance, calls, pages = self.full_fixture()
                pages[endpoint] = (kind, {})
                with self.assertRaisesRegex(worker.LoginError, "^" + expected + "$"):
                    instance.run()
                self.assertEqual(instance.stage, stage)
                self.assertFalse(any(url.endswith("/oauth/token") for _, url, _ in calls))

    def test_excel_unknown_html_requires_manual_authorization(self):
        instance, calls, _ = self.full_fixture()

        def not_json():
            raise ValueError("synthetic HTML page")

        def fetch(method, url, **kwargs):
            calls.append((method, url, kwargs))
            result = response(worker.AUTH + "/interactive-step")
            result.json = not_json
            return result

        instance.fetch = fetch
        with self.assertRaisesRegex(worker.LoginError, "^interaction_required$"):
            instance.run()
        self.assertEqual(instance.stage, "authorize")
        self.assertEqual(len(calls), 1)

    def test_excel_locked_workspace_is_checked_even_without_a_workspace_page(self):
        instance, calls = self.fixture(workspace="other-team")
        instance.selected = None
        instance.request["workspace_id"] = "team-id"
        with self.assertRaisesRegex(worker.LoginError, "^identity_mismatch$"):
            instance.run()
        self.assertEqual(instance.stage, "credential_validation")
        self.assertEqual(len(calls), 1)

    def test_excel_access_failure_never_returns_a_credential(self):
        instance, calls, _ = self.full_fixture(allowed=False)
        with patch.object(worker.time, "time", return_value=60):
            with self.assertRaisesRegex(worker.LoginError, "^verification_failed$"):
                instance.run()
        self.assertEqual(instance.stage, "access_check")
        self.assertEqual(len(calls), 8)

    def test_failure_diagnostics_are_bounded_and_excel_only(self):
        cases = [
            ("excel", "mfa", worker.LoginError("mfa_factor_missing"),
             {"ok": False, "code": "mfa_factor_missing", "stage": "mfa"}),
            ("excel", "token_exchange", RuntimeError("sensitive fixture detail"),
             {"ok": False, "code": "transport_failed", "stage": "token_exchange"}),
            ("excel", "untrusted fixture detail", worker.LoginError("interaction_required"),
             {"ok": False, "code": "interaction_required"}),
            ("codex", "mfa", worker.LoginError("mfa_factor_missing"),
             {"ok": False, "code": "mfa_factor_missing"}),
        ]
        for target, stage, error, expected in cases:
            with self.subTest(target=target, stage=stage):
                instance = login()
                instance.request["credential_target"] = target
                instance.stage = stage
                instance.session.close = lambda: None

                def fail():
                    raise error

                instance.run = fail
                output = io.StringIO()
                stdin = types.SimpleNamespace(buffer=io.BytesIO(b"{}"))
                with patch.object(worker, "Login", return_value=instance), \
                        patch.object(worker.sys, "stdin", stdin), patch.object(worker.sys, "stdout", output):
                    worker.main()
                self.assertEqual(json.loads(output.getvalue()), expected)

    def test_codex_token_and_wrong_workspace_are_rejected(self):
        for kwargs, expected in [({"client": worker.CLIENT}, "invalid_token"),
                                 ({"workspace": "other-team"}, "identity_mismatch"),
                                 ({"allowed": False}, "verification_failed")]:
            with self.subTest(kwargs=kwargs):
                instance, _ = self.fixture(**kwargs)
                with self.assertRaisesRegex(worker.LoginError, "^" + expected + "$"):
                    instance.run_excel_oauth()

    def test_excel_callback_is_captured_without_fetching_it(self):
        for suffix in ["?code=fixture&state=expected", "?state=expected&code=fixture"]:
            instance = login()
            instance.request["credential_target"] = "excel"
            calls = []

            def fetch(method, url, **kwargs):
                calls.append(url)
                return response(url, location=worker.EXCEL_CALLBACK + suffix)

            instance.fetch = fetch
            self.assertEqual(instance.flow(worker.AUTH + "/api/accounts/authorize", oauth=True, state="expected"), "fixture")
            self.assertEqual(calls, [worker.AUTH + "/api/accounts/authorize"])

    def test_excel_callback_rejects_duplicate_and_wrong_state(self):
        for suffix in ["?code=fixture&state=wrong", "?code=fixture&code=second&state=expected",
                       "?code=fixture&state=expected&state=expected", "?code=&state=expected",
                       "?code=fixture&state=expected&error=denied"]:
            instance = login()
            instance.request["credential_target"] = "excel"
            instance.fetch = lambda method, url, **kwargs: response(url, location=worker.EXCEL_CALLBACK + suffix)
            with self.assertRaisesRegex(worker.LoginError, "^callback_mismatch$"):
                instance.flow(worker.AUTH + "/api/accounts/authorize", oauth=True, state="expected")


if __name__ == "__main__":
    unittest.main()
