"""Offline protocol fixtures. Never contacts an upstream or uses real credentials."""
import base64
import importlib.util
import json
from pathlib import Path
import time
import types
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "relogin_worker", Path(__file__).parents[1] / "src/credential/relogin_worker.py"
)
worker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(worker)


def response(url, body=None, location=None):
    return types.SimpleNamespace(
        url=url, json=lambda: body or {}, status_code=302 if location else 200,
        headers={"location": location} if location else {},
    )


def offered_password_page(url):
    return response(url, {"page": {"type": "email_otp_verification",
                                    "payload": {"login_methods": [{"type": "password"}]}}})


def token(payload):
    encoded = base64.urlsafe_b64encode(json.dumps(payload).encode()).decode().rstrip("=")
    return "fixture." + encoded + ".unsigned"


def login():
    instance = object.__new__(worker.Login)
    instance.request = {"email": "test@example.invalid", "password": "test-only-password"}
    instance.selected = {"id": "team-id", "plan": "team"}
    instance.session = types.SimpleNamespace(cookies=types.SimpleNamespace(jar=[]))
    instance.totp = types.SimpleNamespace(now=lambda: "123456")
    instance.did = "fixture-device"
    instance.sid = "fixture-session"
    instance.deadline = time.monotonic() + 60
    instance.proxy = None
    instance.verified_session = None
    instance.rejected_navigation = set()
    instance.effect_started = False
    instance.web_login_performed = False
    return instance


class WorkspaceTests(unittest.TestCase):
    def test_known_plan_aliases_normalize_to_policy_tiers(self):
        aliases = {
            "prolite": "pro",
            "team": "business",
            "self_serve_business_prolite": "business",
            "self_serve_business_usage_based": "business",
            "education": "edu",
            "edu_plus": "edu",
            "edu_pro": "edu",
            "ent26": "enterprise",
            "enterprise_cbp_automation": "enterprise",
            "enterprise_cbp_usage_based": "enterprise",
            "hc": "enterprise",
        }
        for raw, expected in aliases.items():
            with self.subTest(raw=raw):
                self.assertEqual(worker.normalize_plan(raw), expected)
        self.assertIsNone(worker.normalize_plan("future-plan"))

    def test_team_is_preferred_over_personal_and_free(self):
        accounts = {name: {"account": {"plan_type": plan}} for name, plan in [
            ("personal", "pro"), ("team-id", "team"), ("free-id", "free"),
        ]}
        self.assertEqual(worker.select_workspace(accounts), {"id": "team-id", "plan": "business"})
        self.assertEqual(worker.select_workspace(accounts, "personal")["id"], "personal")

    def test_free_only_and_explicit_workspace(self):
        accounts = [{"id": "free-id", "plan_type": "free"}]
        self.assertEqual(worker.select_workspace(accounts)["plan"], "free")
        with self.assertRaisesRegex(worker.LoginError, "workspace_missing"):
            worker.select_workspace(accounts, "missing-team")

    def test_membership_aliases_are_one_workspace(self):
        accounts = {
            "default": {"account": {"account_id": "free-id", "plan_type": "free"}},
            "free-id": {"account": {"account_id": "free-id", "plan_type": "free"}},
        }
        for preferred in (None, "free-id"):
            self.assertEqual(worker.select_workspace(accounts, preferred),
                             {"id": "free-id", "plan": "free"})

    def test_duplicate_memberships_do_not_change_plan_priority(self):
        accounts = [
            {"id": "free-id", "plan_type": "free"},
            {"id": "team-id", "plan_type": "team"},
            {"id": "team-id", "plan_type": "team"},
        ]
        self.assertEqual(worker.select_workspace(accounts), {"id": "team-id", "plan": "business"})

    def test_team_business_aliases_merge_independently_of_order(self):
        accounts = [
            {"id": "team-id", "plan_type": "team"},
            {"id": "team-id", "plan_type": "self_serve_business_prolite"},
        ]
        for entries in (accounts, accounts[::-1]):
            self.assertEqual(worker.select_workspace(entries),
                             {"id": "team-id", "plan": "business"})

    def test_business_and_enterprise_aliases_keep_paid_priority(self):
        accounts = [
            {"id": "free-id", "plan_type": "free"},
            {"id": "pro-id", "plan_type": "prolite"},
            {"id": "business-id", "plan_type": "self_serve_business_prolite"},
        ]
        self.assertEqual(worker.select_workspace(accounts),
                         {"id": "business-id", "plan": "business"})
        accounts.append({"id": "enterprise-id", "plan_type": "ent26"})
        self.assertEqual(worker.select_workspace(accounts),
                         {"id": "enterprise-id", "plan": "enterprise"})

    def test_conflicting_duplicate_plans_fail_closed(self):
        accounts = [
            {"id": "team-id", "plan_type": "free"},
            {"id": "team-id", "plan_type": "team"},
        ]
        for preferred in (None, "team-id"):
            with self.assertRaisesRegex(worker.LoginError, "workspace_plan_unknown"):
                worker.select_workspace(accounts, preferred)

    def test_ambiguous_or_unknown_plan_never_silently_downgrades(self):
        for accounts in [
            [{"id": "a", "plan_type": "team"}, {"id": "b", "plan_type": "business"}],
            [{"id": "a", "plan_type": "future"}, {"id": "b", "plan_type": "free"}],
            [], {"a": {"account": None}},
        ]:
            with self.assertRaises(worker.LoginError):
                worker.select_workspace(accounts)


class FlowTests(unittest.TestCase):
    def run_flow(self, bodies, oauth=True, instance=None):
        instance = instance or login()
        requests = []
        results = iter(bodies)

        def fetch(method, url, **kwargs):
            requests.append((method, url, kwargs))
            return next(results)

        instance.fetch = fetch
        with patch.object(worker.time, "time", return_value=60):
            result = instance.flow(worker.AUTH + "/log-in", oauth=oauth, state="expected-state")
        return result, requests

    def test_password_mfa_workspace_organization_and_consent(self):
        body = lambda kind, payload={}: {"page": {"type": kind, "payload": payload}}
        responses = [
            response(worker.AUTH + "/log-in"),
            response(worker.AUTH + "/api/accounts/authorize/continue",
                     body("login_password", {"url": worker.AUTH + "/log-in/password"})),
            response(worker.AUTH + "/api/accounts/password/verify",
                     body("mfa_challenge", {"factors": [None, {"factor_type": "totp", "id": "factor"}]})),
            response(worker.AUTH + "/api/accounts/mfa/issue_challenge", {}),
            response(worker.AUTH + "/api/accounts/mfa/verify",
                     body("codex_consent", {"url": worker.AUTH + "/sign-in-with-chatgpt/codex/consent"})),
            response(worker.AUTH + "/api/accounts/workspace/select",
                     body("codex_organization", {"organizations": [{"id": "org"}], "projects": [{"id": "project"}]})),
            response(worker.AUTH + "/api/accounts/organization/select", body("consent")),
            response(worker.AUTH + "/api/accounts/consent/grant",
                     location=worker.CALLBACK + "?code=fixture-code&state=expected-state"),
        ]
        result, requests = self.run_flow(responses)
        self.assertEqual(result, "fixture-code")
        self.assertEqual(len(requests), 8)
        self.assertEqual(requests[2][2]["json"], {"password": "test-only-password"})
        self.assertEqual(requests[3][2]["json"], {"id": "factor", "type": "totp", "force_fresh_challenge": False})
        self.assertEqual(requests[4][2]["json"], {"id": "factor", "type": "totp", "code": "123456"})
        self.assertEqual(requests[5][2]["json"], {"workspace_id": "team-id"})
        self.assertEqual(requests[6][2]["json"], {"org_id": "org", "project_id": "project"})
        self.assertTrue(all(not url.startswith("http://localhost") for _, url, _ in requests))

    def test_initial_email_login_can_select_password_then_complete_totp(self):
        for password_body, html in [
            ({"page": {"type": "login_password"}}, ""),
            ({}, '<form><input type="password" name="password"></form>'),
        ]:
            with self.subTest(password_body=password_body):
                password_page = response(worker.AUTH + "/log-in/password", password_body)
                password_page.text = html
                result, calls = self.run_flow([
                    response(worker.AUTH + "/oauth/authorize", {"page": {
                        "type": "email_otp_verification", "payload": {
                            "email_verification_mode": "passwordless_login",
                            "login_methods": [{"type": "passwordless_otp"}, {"type": "password"}],
                        },
                    }}),
                    password_page,
                    response(worker.AUTH + "/api/accounts/password/verify", {"page": {
                        "type": "mfa_challenge", "payload": {"factor_id": "factor"},
                    }}),
                    response(worker.AUTH + "/api/accounts/mfa/verify", location=worker.WEB + "/"),
                    response(worker.WEB + "/"),
                ], oauth=False)
                self.assertIsNone(result)
                self.assertEqual([url for _, url, _ in calls], [
                    worker.AUTH + "/log-in", worker.AUTH + "/log-in/password",
                    worker.AUTH + "/api/accounts/password/verify",
                    worker.AUTH + "/api/accounts/mfa/verify", worker.WEB + "/",
                ])
                self.assertEqual(calls[2][2]["json"], {"password": "test-only-password"})
                self.assertEqual(calls[3][2]["json"]["code"], "123456")
                self.assertIn("text/html", calls[1][2]["headers"]["Accept"])

    def test_password_entry_redirect_keeps_html_navigation_headers(self):
        instance = login()
        calls = []
        def fetch(method, url, **kwargs):
            calls.append((method, url))
            if url.endswith("/oauth/authorize"):
                return offered_password_page(url)
            if url.endswith("/log-in/password"):
                self.assertIn("text/html", kwargs["headers"]["Accept"])
                return response(url, location=worker.AUTH + "/log-in/password?step=password")
            if "step=password" in url:
                result = response(url)
                # Match upstream content negotiation: JSON Accept receives no form.
                result.text = '<input type="password">' if "text/html" in kwargs.get("headers", {}).get("Accept", "") else ""
                return result
            if url.endswith("/api/accounts/password/verify"):
                return response(url, {"page": {"type": "email_otp_verification"}})
            self.fail("unexpected authentication request")
        instance.fetch = fetch
        with self.assertRaisesRegex(worker.LoginError, "^email_verification_required$"):
            instance.flow(worker.AUTH + "/oauth/authorize")
        self.assertEqual(len(calls), 4)

    def test_password_entry_is_attempted_once_and_must_be_confirmed(self):
        challenge = response(worker.AUTH + "/log-in/password")
        challenge.text = '<title>Just a moment...</title><input type="password">'
        for page, expected in [
            (response(worker.AUTH + "/email-verification"), "email_verification_required"),
            (response(worker.AUTH + "/log-in/password"), "interaction_required"),
            (response(worker.AUTH + "/phone-verification"), "interaction_required"),
            (challenge, "interaction_required"),
        ]:
            with self.subTest(url=page.url):
                instance = login()
                calls = []
                results = iter([offered_password_page(worker.AUTH + "/email-verification"), page])
                def fetch(method, url, **kwargs):
                    calls.append((method, url))
                    return next(results)
                instance.fetch = fetch
                with self.assertRaisesRegex(worker.LoginError, "^" + expected + "$"):
                    instance.flow(worker.AUTH + "/oauth/authorize")
                self.assertEqual(calls, [
                    ("GET", worker.AUTH + "/oauth/authorize"),
                    ("GET", worker.AUTH + "/log-in/password"),
                ])

    def test_password_entry_does_not_follow_a_different_origin(self):
        instance = login()
        calls = []
        results = iter([
            offered_password_page(worker.AUTH + "/email-verification"),
            response(worker.AUTH + "/log-in/password", location=worker.WEB + "/"),
        ])
        def fetch(method, url, **kwargs):
            calls.append((method, url))
            return next(results)
        instance.fetch = fetch
        with self.assertRaisesRegex(worker.LoginError, "^unexpected_redirect$"):
            instance.flow(worker.AUTH + "/oauth/authorize")
        self.assertEqual(len(calls), 2)

    def test_email_login_without_an_offered_password_stops_on_first_response(self):
        for payload in ({}, {"login_methods": [{"type": "passwordless_otp"}]}, {"login_methods": "password"}):
            with self.subTest(payload=payload):
                instance = login()
                body = {"page": {"type": "email_otp_verification", "payload": payload},
                        "oai-client-auth-session": {"login_methods": [{"type": "password"}]}}
                with patch.object(instance, "fetch", return_value=response(worker.AUTH + "/email-verification", body)) as fetch:
                    with self.assertRaisesRegex(worker.LoginError, "^email_verification_required$"):
                        instance.flow(worker.AUTH + "/oauth/authorize")
                    fetch.assert_called_once_with("GET", worker.AUTH + "/oauth/authorize")

    def test_email_after_password_or_totp_never_restarts_password_login(self):
        for initial, endpoint in [
            ({"type": "login_password"}, "/api/accounts/password/verify"),
            ({"type": "mfa_challenge", "payload": {"factor_id": "factor"}}, "/api/accounts/mfa/verify"),
        ]:
            with self.subTest(initial=initial):
                instance = login()
                calls = []
                results = iter([
                    response(worker.AUTH + "/oauth/authorize", {"page": initial}),
                    offered_password_page(worker.AUTH + endpoint),
                ])
                def fetch(method, url, **kwargs):
                    calls.append((method, url))
                    return next(results)
                instance.fetch = fetch
                with patch.object(worker.time, "time", return_value=60):
                    with self.assertRaisesRegex(worker.LoginError, "^email_verification_required$"):
                        instance.flow(worker.AUTH + "/oauth/authorize")
                self.assertEqual(calls, [("GET", worker.AUTH + "/oauth/authorize"), ("POST", worker.AUTH + endpoint)])

    def test_email_challenges_in_oauth_stop_without_sending_or_reading_codes(self):
        for kind in ("email_otp_send", "email_otp_verification"):
            with self.subTest(kind=kind):
                instance = login()
                calls = []

                def fetch(method, url, **kwargs):
                    calls.append((method, url))
                    return response(worker.AUTH, {"page": {"type": kind}})

                instance.fetch = fetch
                with self.assertRaisesRegex(worker.LoginError, "email_verification_required"):
                    instance.flow(worker.AUTH + "/log-in", oauth=True)
                self.assertEqual(calls, [("GET", worker.AUTH + "/log-in")])

    def test_invalid_or_legacy_material_stops_before_creating_a_session(self):
        for secret in (None, "", "invalid-secret", "123e4567-e89b-12d3-a456-426614174000"):
            with self.subTest(secret=secret):
                with self.assertRaisesRegex(worker.LoginError, "invalid_material"):
                    worker.Login({
                        "email": "test@example.invalid",
                        "password": "test-only-password",
                        "mfa_secret": secret,
                        "mailbox": {"client_id": "unused", "refresh_token": "unused"},
                    })

    def test_initialization_uses_totp_only(self):
        session = types.SimpleNamespace(cookies=types.SimpleNamespace(set=lambda *a, **k: None))
        requests = types.SimpleNamespace(Session=lambda **kwargs: session)
        totp = types.SimpleNamespace(TOTP=lambda secret: ("totp", secret))
        with patch.dict("sys.modules", {"curl_cffi": types.SimpleNamespace(requests=requests, CurlOpt=types.SimpleNamespace(PROXY=1, NOPROXY=2)),
                                       "pyotp": totp}):
            instance = worker.Login({
                "email": "test@example.invalid", "password": "test-only-password",
                "mfa_secret": "JBSWY3DPEHPK3PXP",
            })
        self.assertEqual(instance.totp, ("totp", "JBSWY3DPEHPK3PXP"))
        self.assertIs(instance.session, session)
        self.assertFalse(hasattr(instance, "mailbox"))

    def test_callback_requires_exact_state_and_one_code(self):
        for query in ["code=a&state=wrong", "code=a&code=b&state=expected-state", "error=denied&state=expected-state"]:
            with self.assertRaisesRegex(worker.LoginError, "callback_mismatch"):
                self.run_flow([response(worker.AUTH, location=worker.CALLBACK + "?" + query)])

    def test_account_selection_uses_unified_session_not_auth_transaction(self):
        result, requests = self.run_flow([
            response(worker.AUTH + "/oauth/authorize", {
                "page": {"type": "choose_an_account"},
                "oai-client-auth-session": {
                    "session_id": "auth-transaction",
                    "unified_sessions": [{"id": "login-session"}],
                },
            }),
            response(worker.AUTH, location=worker.CALLBACK + "?code=a&state=expected-state"),
        ])
        self.assertEqual(result, "a")
        self.assertEqual(requests[1][2]["json"], {"session_id": "login-session"})

    def test_auth_transaction_cannot_resolve_missing_or_ambiguous_sessions(self):
        for choices in ([], [{"id": "a"}, {"id": "b"}]):
            with self.assertRaisesRegex(worker.LoginError, "interaction_required"):
                self.run_flow([response(worker.AUTH, {
                    "page": {"type": "choose_an_account"},
                    "oai-client-auth-session": {
                        "session_id": "auth-transaction", "unified_sessions": choices,
                    },
                })])

    def test_interactive_challenges_stop(self):
        for kind in ["captcha", "add_phone", "unknown_page"]:
            with self.assertRaisesRegex(worker.LoginError, "interaction_required"):
                self.run_flow([response(worker.AUTH, {"page": {"type": kind}})])

    def test_missing_factor_stops(self):
        with self.assertRaisesRegex(worker.LoginError, "mfa_factor_missing"):
            self.run_flow([response(worker.AUTH, {"page": {"type": "mfa_challenge"}})])

    def test_workspace_consent_html_uses_locked_workspace(self):
        result, requests = self.run_flow([
            response(worker.AUTH + "/sign-in-with-chatgpt/codex/consent"),
            response(worker.AUTH, location=worker.CALLBACK + "?code=a&state=expected-state"),
        ])
        self.assertEqual(result, "a")
        self.assertEqual(requests[1][2]["json"], {"workspace_id": "team-id"})

    def test_organization_ambiguity_requires_interaction(self):
        with self.assertRaisesRegex(worker.LoginError, "interaction_required"):
            worker.unique_choice({"organizations": [{"id": "a"}, {"id": "b"}]}, (), ("organizations",), ("id",))

    def test_url_allowlist(self):
        self.assertEqual(worker.safe_url(worker.AUTH + "/log-in"), worker.AUTH + "/log-in")
        for url in ["http://auth.openai.com/", "https://example.invalid/", "https://auth.openai.com.evil.invalid/",
                    "https://user:password@auth.openai.com/", "https://chatgpt.com:8080/", "file:///tmp/local"]:
            with self.assertRaises(worker.LoginError):
                worker.safe_url(url)

    def test_fetch_keeps_tls_and_redirect_checks(self):
        instance = login()
        calls = []
        instance.session.request = lambda *args, **kwargs: (
            calls.append((args, kwargs)) or types.SimpleNamespace(content=b"{}", status_code=200)
        )
        instance.fetch("GET", worker.AUTH + "/log-in")
        self.assertIs(calls[0][1]["verify"], True)
        self.assertIs(calls[0][1]["allow_redirects"], False)


class CredentialTests(unittest.TestCase):
    def run_login(self, override=None, usage_override=None, identity_override=None,
                  membership_plan="team"):
        instance = login()
        instance.request["workspace_id"] = None
        instance.flow = lambda url, **kwargs: "fixture-code"
        auth = {"chatgpt_account_id": "team-id", "chatgpt_user_id": "user-id", "chatgpt_plan_type": "team"}
        auth.update(override or {})
        tokens = {
            "access_token": token({"exp": time.time() + 3600, "https://api.openai.com/auth": auth}),
            "id_token": token({"email": "test@example.invalid", "https://api.openai.com/auth": auth | (identity_override or {})}),
            "refresh_token": "test-only-refresh",
        }
        bodies = iter([
            {"csrfToken": "fixture-csrf"}, {"url": worker.AUTH + "/log-in"},
            {"accessToken": "fixture-web-token", "user": {"email": "test@example.invalid"}},
            {"accounts": {"personal": {"account": {"plan_type": "free"}},
                          "team-id": {"account": {"plan_type": membership_plan}}}},
            tokens, usage_override if usage_override is not None else {"rate_limit": {}, "plan_type": "team"},
        ])
        instance.fetch = lambda method, url, **kwargs: response(url, next(bodies))
        return instance.run()

    def test_verified_team_document_is_returned_without_password_or_mfa(self):
        result = self.run_login()
        self.assertTrue(result["ok"])
        self.assertEqual(result["document"]["account_id"], "team-id")
        self.assertNotIn("password", json.dumps(result))
        self.assertNotIn("mfa", json.dumps(result))

    def test_business_aliases_agree_across_membership_claims_and_usage(self):
        result = self.run_login(
            {"chatgpt_plan_type": "self_serve_business_prolite"},
            {"rate_limit": {}, "plan_type": "team"},
            {"chatgpt_plan_type": "business"},
            membership_plan="self_serve_business_usage_based",
        )
        self.assertTrue(result["ok"])
        self.assertEqual(result["document"]["account_id"], "team-id")

    def test_wrong_workspace_or_downgraded_plan_never_returns_document(self):
        for override in [
            {"chatgpt_account_id": "personal"},
            {"chatgpt_plan_type": "free"},
            {"chatgpt_plan_type": "future-plan"},
        ]:
            with self.assertRaises(worker.LoginError):
                self.run_login(override)

    def test_token_claims_must_agree_on_identity_and_plan(self):
        for identity in [{"chatgpt_account_id": "other"}, {"chatgpt_user_id": "other"}, {"chatgpt_plan_type": "free"}]:
            with self.assertRaises(worker.LoginError):
                self.run_login(identity_override=identity)

    def test_usage_must_be_recognizable_and_match_workspace_and_plan(self):
        for usage in [
            {},
            {"rate_limit": {}, "account_id": "other"},
            {"rate_limit": {}, "plan_type": "free"},
            {"rate_limit": {}, "plan_type": "future-plan"},
        ]:
            with self.assertRaises(worker.LoginError):
                self.run_login(usage_override=usage)

    def test_invalid_token_is_rejected(self):
        for value in ["", "x.y.z", token([])]:
            with self.assertRaises(worker.LoginError):
                worker.claims(value)


if __name__ == "__main__":
    unittest.main()
