"""Isolated OAuth login worker. One request on stdin, one result on stdout.

Protocol stages follow the supplied ReloginDesktop OAuth implementation.
Challenges requiring a browser, email or phone are terminal here, never bypassed.
"""

import base64
import hashlib
import json
import secrets
import sys
import time
import uuid
from urllib.parse import parse_qs, urlencode, urljoin, urlparse

AUTH = "https://auth.openai.com"
WEB = "https://chatgpt.com"
CLIENT = "app_EMoamEEZ73f0CkXaXp7hrann"
CALLBACK = "http://localhost:1455/auth/callback"
# Business policy, not a universal comparison of subscriptions.
PLAN_PRIORITY = {
    "enterprise": 60, "edu": 55, "business": 50,
    "pro": 40, "plus": 30, "go": 20, "free": 10,
}
PLAN_ALIASES = {
    "free": "free", "go": "go", "plus": "plus", "pro": "pro", "prolite": "pro",
    "team": "business", "business": "business",
    "self_serve_business_prolite": "business",
    "self_serve_business_usage_based": "business",
    "edu": "edu", "education": "edu", "edu_plus": "edu", "edu_pro": "edu",
    "enterprise": "enterprise", "hc": "enterprise", "ent26": "enterprise",
    "enterprise_cbp_automation": "enterprise",
    "enterprise_cbp_usage_based": "enterprise",
}


class LoginError(Exception):
    pass


class WorkspaceSelectionRequired(LoginError):
    def __init__(self, choices):
        super().__init__("workspace_ambiguous")
        self.choices = choices


def claims(token):
    try:
        payload = token.split(".")[1]
        value = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
        if not isinstance(value, dict):
            raise ValueError()
        return value
    except (ValueError, IndexError, TypeError):
        raise LoginError("invalid_token") from None


def normalize_plan(value):
    return PLAN_ALIASES.get(str(value or "").strip().lower())


def select_workspace(accounts, preferred=None):
    by_id = {}
    names = {}
    if isinstance(accounts, dict):
        iterable = accounts.items()
    elif isinstance(accounts, list):
        iterable = [(None, item) for item in accounts]
    else:
        raise LoginError("workspace_unknown")
    for key, item in iterable:
        if not isinstance(item, dict):
            continue
        account = item.get("account", item)
        if not isinstance(account, dict):
            continue
        identifier = account.get("account_id") or account.get("id") or key
        plan = normalize_plan(account.get("plan_type"))
        if identifier:
            identifier = str(identifier)
            previous = by_id.get(identifier)
            if previous and previous["plan"] != plan:
                raise LoginError("workspace_plan_unknown")
            # The membership map may expose the same workspace under an alias.
            by_id[identifier] = {"id": identifier, "plan": plan}
            name = account.get("name")
            if isinstance(name, str) and name.strip():
                names[identifier] = "".join(char for char in name if char.isprintable()).strip()[:128]
    items = list(by_id.values())
    if preferred:
        matches = [item for item in items if item["id"] == preferred]
        if len(matches) != 1:
            raise LoginError("workspace_missing")
        return matches[0]
    if not items or any(item["plan"] not in PLAN_PRIORITY for item in items):
        raise LoginError("workspace_plan_unknown")
    priority = max(PLAN_PRIORITY[item["plan"]] for item in items)
    best = [item for item in items if PLAN_PRIORITY[item["plan"]] == priority]
    if len(best) != 1:
        if len(best) > 64 or any(
            len(item["id"]) > 128 or not item["id"]
            or any(char.isspace() or not char.isprintable() for char in item["id"])
            for item in best
        ):
            raise LoginError("workspace_unknown")
        raise WorkspaceSelectionRequired([
            {"id": item["id"], "name": names.get(item["id"], ""), "planType": item["plan"]}
            for item in sorted(best, key=lambda item: item["id"])
        ])
    return best[0]


def safe_url(url):
    parsed = urlparse(url)
    if (parsed.scheme != "https" or parsed.hostname not in ("auth.openai.com", "chatgpt.com")
            or parsed.username or parsed.password or parsed.port not in (None, 443)):
        raise LoginError("unexpected_redirect")
    return url


def unique_choice(data, scalar_keys, list_keys, id_keys):
    for key in scalar_keys:
        if data.get(key):
            return data[key]
    values = set()
    for key in list_keys:
        choices = data.get(key)
        if isinstance(choices, list):
            for choice in choices:
                if isinstance(choice, dict):
                    value = next((choice[key] for key in id_keys if choice.get(key)), None)
                    if isinstance(value, str):
                        values.add(value)
    if len(values) != 1:
        raise LoginError("interaction_required")
    return values.pop()


class Login:
    def __init__(self, request):
        secret = request.get("mfa_secret")
        if (not request.get("password") or not isinstance(secret, str)
                or not 16 <= len(secret) <= 128 or len(secret) % 8 not in (0, 2, 4, 5, 7)
                or any(ch not in "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567" for ch in secret)):
            raise LoginError("invalid_material")
        from curl_cffi import requests
        import pyotp
        self.request = request
        self.totp = pyotp.TOTP(secret)
        self.session = requests.Session(impersonate="firefox133")
        self.proxy = request.get("proxy")
        self.did = str(uuid.uuid4())
        self.selected = None
        self.deadline = time.monotonic() + 240
        self.session.cookies.set("oai-did", self.did, domain="auth.openai.com")
        self.session.cookies.set("oai-did", self.did, domain="chatgpt.com")

    def fetch(self, method, url, **kwargs):
        safe_url(url)
        if time.monotonic() > self.deadline:
            raise LoginError("timeout")
        headers = {
            "Accept": "application/json",
            "Origin": AUTH if url.startswith(AUTH) else WEB,
            "Referer": AUTH + "/log-in" if url.startswith(AUTH) else WEB + "/",
            "oai-device-id": self.did,
        }
        headers.update(kwargs.pop("headers", {}))
        response = self.session.request(
            method, url, headers=headers, timeout=30, proxy=self.proxy,
            verify=True, allow_redirects=False, **kwargs,
        )
        if len(response.content) > 2 * 1024 * 1024:
            raise LoginError("response_too_large")
        if response.status_code >= 400:
            path = urlparse(url).path
            if response.status_code == 429:
                raise LoginError("rate_limited")
            error = self.body(response).get("error")
            if response.status_code in (400, 401, 403) and isinstance(error, dict):
                code = error.get("code")
                if code in ("account_banned", "account_deactivated", "account_disabled", "account_suspended"):
                    raise LoginError("account_banned")
                if code in ("deactivated_workspace", "workspace_deactivated", "organization_disabled"):
                    raise LoginError("workspace_unavailable")
            if "password/verify" in path:
                raise LoginError("password_rejected")
            if "mfa/verify" in path:
                raise LoginError("mfa_rejected")
            raise LoginError("upstream_rejected")
        return response

    @staticmethod
    def body(response):
        try:
            value = response.json()
            return value if isinstance(value, dict) else {}
        except ValueError:
            return {}

    def session_data(self, body):
        data = {}
        for cookie in self.session.cookies.jar:
            if cookie.name == "oai-client-auth-session":
                try:
                    encoded = cookie.value.split(".")[0]
                    decoded = json.loads(base64.urlsafe_b64decode(encoded + "=" * (-len(encoded) % 4)))
                    if isinstance(decoded, dict):
                        data.update(decoded)
                except (ValueError, TypeError):
                    pass
        embedded = body.get("oai-client-auth-session")
        if isinstance(embedded, dict):
            data.update(embedded)
        page = body.get("page") or {}
        if isinstance(page, dict) and isinstance(page.get("payload"), dict):
            data.update(page["payload"])
        return data

    def flow(self, url, oauth=False, state=None):
        response = self.fetch("GET", url)
        counts = {}
        for _ in range(32):
            body = self.body(response)
            location = response.headers.get("location")
            page = body.get("page") or {}
            page_type = page.get("type") if isinstance(page, dict) else ""
            data = self.session_data(body)
            # Payload URLs usually describe the current page, not a redirect.
            if not page_type or page_type == "external_url":
                location = location or body.get("continue_url") or data.get("url")
            if location:
                location = urljoin(str(response.url), location)
                parsed = urlparse(location)
                if (oauth and parsed.scheme == "http" and parsed.hostname == "localhost"
                        and parsed.port == 1455 and parsed.path == "/auth/callback"
                        and not parsed.username and not parsed.password and not parsed.fragment):
                    query = parse_qs(parsed.query)
                    if query.get("state") != [state] or len(query.get("code", [])) != 1:
                        raise LoginError("callback_mismatch")
                    return query["code"][0]
                response = self.fetch("GET", location)
                continue
            if not oauth and str(response.url).startswith(WEB) and response.status_code == 200:
                return None
            path = urlparse(str(response.url)).path
            if not page_type:
                page_type = {
                    "/log-in": "login_identifier",
                    "/login": "login_identifier",
                    "/log-in/password": "login_password",
                    "/mfa-challenge": "mfa_challenge",
                    "/sign-in-with-chatgpt/codex/consent": "codex_consent",
                    "/sign-in-with-chatgpt/codex/organization": "codex_organization",
                    "/consent": "consent",
                    "/choose-an-account": "choose_an_account",
                }.get(path, "")
                if path.startswith("/mfa-challenge/"):
                    page_type = "mfa_challenge"
                    data.setdefault("factor_id", path.rsplit("/", 1)[-1])
            counts[page_type] = counts.get(page_type, 0) + 1
            if counts[page_type] > 2:
                raise LoginError("login_loop")
            if page_type in ("login_identifier", "login", "email"):
                endpoint = "/api/accounts/authorize/continue"
                payload = {"username": {"kind": "email", "value": self.request["email"]}}
            elif page_type == "login_password":
                endpoint = "/api/accounts/password/verify"
                payload = {"password": self.request["password"]}
            elif page_type == "mfa_challenge":
                factor = data.get("factor_id")
                factors = data.get("factors") or data.get("mfa_challenge_factors") or []
                if not factor:
                    factor = next((item.get("id") or item.get("factor_id") for item in factors
                                   if isinstance(item, dict) and item.get("factor_type") == "totp"), None)
                if not factor:
                    raise LoginError("mfa_factor_missing")
                if time.time() % 30 > 26:
                    time.sleep(30 - time.time() % 30 + 0.1)
                endpoint = "/api/accounts/mfa/verify"
                payload = {"id": factor, "type": "totp", "code": self.totp.now()}
            elif page_type in ("workspace", "workspace_select", "sign_in_with_chatgpt_codex",
                               "sign_in_with_chatgpt_codex_consent", "codex_consent"):
                choices = data.get("workspaces") or data.get("workspace_choices") or data.get("available_workspaces")
                preferred = self.selected["id"] if self.selected else self.request.get("workspace_id")
                if oauth:
                    if not preferred:
                        raise LoginError("workspace_unknown")
                    workspace = preferred
                else:
                    # This web session is used only to enumerate memberships, never exported.
                    workspace = preferred or data.get("current_workspace_id") or data.get("workspace_id")
                    if not workspace and isinstance(choices, list) and choices:
                        workspace = choices[0].get("id") or choices[0].get("workspace_id")
                if not workspace:
                    raise LoginError("workspace_unknown")
                endpoint = "/api/accounts/workspace/select"
                payload = {"workspace_id": workspace}
            elif page_type in ("sign_in_with_chatgpt_codex_organization", "codex_organization"):
                endpoint = "/api/accounts/organization/select"
                payload = {
                    "org_id": unique_choice(data, ("current_organization_id", "organization_id", "org_id"),
                                            ("organizations", "orgs", "organization_choices"),
                                            ("id", "organization_id", "org_id")),
                    "project_id": unique_choice(data, ("current_project_id", "project_id"),
                                                ("projects", "project_choices"), ("id", "project_id")),
                }
            elif page_type == "choose_an_account":
                endpoint = "/api/accounts/session/select"
                # session_id identifies this authorization flow, not a login choice.
                payload = {"session_id": unique_choice(data, ("selected_session_id",),
                                                       ("unified_sessions",), ("id", "session_id"))}
            elif page_type in ("sign_in_with_chatgpt_consent", "consent"):
                endpoint = "/api/accounts/consent/grant"
                payload = None
            else:
                raise LoginError("interaction_required")
            referer = {
                "/api/accounts/authorize/continue": "/log-in",
                "/api/accounts/password/verify": "/log-in/password",
                "/api/accounts/mfa/verify": "/mfa-challenge",
                "/api/accounts/workspace/select": "/sign-in-with-chatgpt/codex/consent",
                "/api/accounts/organization/select": "/sign-in-with-chatgpt/codex/organization",
                "/api/accounts/session/select": "/choose-an-account",
                "/api/accounts/consent/grant": "/consent",
            }[endpoint]
            response = self.fetch(
                "POST", AUTH + endpoint,
                **({"json": payload} if payload is not None else {}),
                headers={"Referer": AUTH + referer},
            )
        raise LoginError("login_loop")

    def run(self):
        csrf = self.body(self.fetch("GET", WEB + "/api/auth/csrf")).get("csrfToken")
        if not csrf:
            raise LoginError("csrf_missing")
        params = urlencode({"prompt": "login", "screen_hint": "login", "login_hint": self.request["email"],
                            "ext-oai-did": self.did, "auth_session_logging_id": str(uuid.uuid4())})
        response = self.fetch("POST", WEB + "/api/auth/signin/openai?" + params,
                              data={"csrfToken": csrf, "callbackUrl": WEB + "/", "json": "true"})
        url = self.body(response).get("url")
        if not url:
            raise LoginError("authorize_missing")
        self.flow(url)
        web_token = self.body(self.fetch("GET", WEB + "/api/auth/session")).get("accessToken")
        if not web_token:
            raise LoginError("session_missing")
        memberships = self.body(self.fetch("GET", WEB + "/backend-api/accounts/check/v4-2023-04-27",
                                          headers={"Authorization": "Bearer " + web_token}))
        self.selected = select_workspace(memberships.get("accounts"), self.request.get("workspace_id"))
        verifier = secrets.token_urlsafe(64)
        challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).decode().rstrip("=")
        state = secrets.token_urlsafe(32)
        authorize = AUTH + "/oauth/authorize?" + urlencode({
            "client_id": CLIENT, "response_type": "code", "redirect_uri": CALLBACK,
            "scope": "openid email profile offline_access", "state": state,
            "code_challenge": challenge, "code_challenge_method": "S256",
            "id_token_add_organizations": "true", "codex_cli_simplified_flow": "true",
        })
        code = self.flow(authorize, oauth=True, state=state)
        tokens = self.body(self.fetch("POST", AUTH + "/oauth/token", data={
            "grant_type": "authorization_code", "client_id": CLIENT, "code": code,
            "redirect_uri": CALLBACK, "code_verifier": verifier,
        }))
        access, identity = claims(tokens.get("access_token", "")), claims(tokens.get("id_token", ""))
        auth = access.get("https://api.openai.com/auth", {})
        id_auth = identity.get("https://api.openai.com/auth", {})
        for key in ("chatgpt_account_id", "chatgpt_user_id"):
            if auth.get(key) and id_auth.get(key) and auth[key] != id_auth[key]:
                raise LoginError("identity_mismatch")
        access_plan_raw = auth.get("chatgpt_plan_type")
        identity_plan_raw = id_auth.get("chatgpt_plan_type")
        access_plan = normalize_plan(access_plan_raw)
        identity_plan = normalize_plan(identity_plan_raw)
        if ((access_plan_raw and not access_plan) or (identity_plan_raw and not identity_plan)
                or (access_plan and identity_plan and access_plan != identity_plan)):
            raise LoginError("plan_mismatch")
        profile_email = access.get("https://api.openai.com/profile", {}).get("email")
        if identity.get("email") and profile_email and identity["email"].lower() != profile_email.lower():
            raise LoginError("identity_mismatch")
        workspace = auth.get("chatgpt_account_id") or id_auth.get("chatgpt_account_id")
        user = auth.get("chatgpt_user_id") or id_auth.get("chatgpt_user_id")
        plan = access_plan or identity_plan
        email = identity.get("email") or profile_email
        if workspace != self.selected["id"] or str(email).lower() != self.request["email"].lower() or not user:
            raise LoginError("identity_mismatch")
        if not plan or (self.selected["plan"] and plan != self.selected["plan"]):
            raise LoginError("plan_mismatch")
        if not tokens.get("refresh_token") or access.get("exp", 0) <= time.time():
            raise LoginError("invalid_token")
        usage = self.body(self.fetch("GET", WEB + "/backend-api/wham/usage",
                                    headers={"Authorization": "Bearer " + tokens["access_token"],
                                             "ChatGPT-Account-Id": workspace}))
        if usage.get("account_id") and usage["account_id"] != workspace:
            raise LoginError("identity_mismatch")
        if usage.get("plan_type"):
            usage_plan = normalize_plan(usage["plan_type"])
            if not usage_plan or usage_plan != plan:
                raise LoginError("plan_mismatch")
        if not any(isinstance(usage.get(key), (dict, list))
                   for key in ("rate_limit", "additional_rate_limits", "spend_control", "credits")):
            raise LoginError("verification_failed")
        document = {key: tokens[key] for key in ("access_token", "refresh_token", "id_token")}
        document.update({"email": email, "account_id": workspace, "type": "codex"})
        return {"ok": True, "document": document}


def main():
    login = None
    try:
        request = json.loads(sys.stdin.buffer.read(16 * 1024 + 1))
        login = Login(request)
        result = login.run()
    except ImportError:
        result = {"ok": False, "code": "runtime_missing"}
    except WorkspaceSelectionRequired as error:
        result = {"ok": False, "code": "workspace_ambiguous", "workspaces": error.choices}
    except LoginError as error:
        result = {"ok": False, "code": str(error)}
    except Exception:
        # Exception text may contain credentials, cookies or upstream bodies.
        result = {"ok": False, "code": "transport_failed"}
    finally:
        if login:
            login.session.close()
    print(json.dumps(result, separators=(",", ":")), flush=True)


if __name__ == "__main__":
    main()
