"""Synthetic Excel OAuth fixtures. No upstream requests or real login material."""

import time
import types
import unittest
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

    def test_excel_uses_independent_authorization_and_access_validation(self):
        instance, calls = self.fixture()
        result = instance.run_excel_oauth()
        self.assertTrue(result["ok"])
        self.assertEqual(result["document"]["type"], "cpa-bps")
        self.assertEqual(result["document"]["chatgpt_account_id"], "team-id")
        self.assertEqual(result["document"]["user_id"], "fixture-user")
        self.assertEqual(len(calls), 2)
        self.assertFalse(any("wham" in url for _, url, _ in calls))

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
