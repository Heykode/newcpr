"""Isolated OAuth login worker. One request on stdin, one result on stdout.

Protocol stages follow the supplied ReloginDesktop OAuth implementation.
Challenges requiring a browser, email or phone are terminal here, never bypassed.
"""

import base64
import hashlib
import json
import re
import secrets
import sys
import time
import uuid
from datetime import datetime, timezone
from http.cookiejar import Cookie
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


def workspace_directory(accounts):
    """Merge aliases without letting an accessible alias erase an explicit denial."""
    if isinstance(accounts, dict):
        iterable = accounts.items()
    elif isinstance(accounts, list):
        iterable = [(None, item) for item in accounts]
    else:
        raise LoginError("workspace_unknown")
    result = {}
    for key, item in iterable:
        if not isinstance(item, dict):
            continue
        account = item.get("account", item)
        if not isinstance(account, dict):
            continue
        identifier = account.get("account_id") or account.get("id") or key
        if not isinstance(identifier, str) or not identifier or identifier == "default":
            continue
        denied = any(
            obj.get("can_access_with_session") is False or obj.get("can_access") is False
            or obj.get("is_deactivated") is True or obj.get("is_removed") is True
            or bool(obj.get("error"))
            for obj in (item, account)
        )
        plan = normalize_plan(account.get("plan_type"))
        previous = result.get(identifier)
        if previous and previous["plan"] and plan and previous["plan"] != plan:
            raise LoginError("workspace_plan_unknown")
        name = account.get("name")
        name = "".join(c for c in name if c.isprintable()).strip()[:128] if isinstance(name, str) else ""
        result[identifier] = {
            "id": identifier, "plan": plan or (previous or {}).get("plan"),
            "name": name or (previous or {}).get("name", ""),
            "available": not denied and (previous or {}).get("available", True),
        }
    return list(result.values())


def select_workspace(accounts, preferred=None):
    items = workspace_directory(accounts)
    if preferred:
        matches = [item for item in items if item["id"] == preferred]
        if len(matches) != 1:
            raise LoginError("workspace_missing")
        if not matches[0]["available"]:
            raise LoginError("workspace_unavailable")
        return {"id": matches[0]["id"], "plan": matches[0]["plan"]}
    items = [item for item in items if item["available"]]
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
            {"id": item["id"], "name": item["name"], "planType": item["plan"]}
            for item in sorted(best, key=lambda item: item["id"])
        ])
    return {"id": best[0]["id"], "plan": best[0]["plan"]}



def totp_factors(info):
    factors = info.get("factors") or info.get("mfa_factors") or []
    grouped = isinstance(factors, dict)
    if grouped:
        factors = factors.get("totp", [])
    if not isinstance(factors, list):
        return []
    return [x for x in factors if isinstance(x, dict)
            and (grouped or (x.get("factor_type") or x.get("type")) == "totp")]


def totp_factor_ids(info):
    ids = {x.get("id") or x.get("factor_id") for x in totp_factors(info)}
    ids = {identifier for identifier in ids if isinstance(identifier, str) and identifier}
    native = info.get("native_default_factor_id")
    if isinstance(native, str) and native:
        ids.add(native)
    return ids


def totp_state(info):
    flags = [info[key] for key in ("mfa_enabled_v2", "mfa_enabled") if isinstance(info.get(key), bool)]
    factor = bool(totp_factors(info) or totp_factor_ids(info))
    if flags and all(flags) and factor:
        return True
    if flags and not any(flags) and not factor:
        return False
    return None


def acknowledged(response, *, allow_null=False):
    if response.status_code == 204:
        return
    if not 200 <= response.status_code < 300:
        raise LoginError("operation_unconfirmed")
    try:
        data = response.json()
    except (ValueError, TypeError):
        raise LoginError("operation_unconfirmed") from None
    if allow_null and data is None:
        return
    if (not isinstance(data, dict) or data.get("error")
            or data.get("success") is False or data.get("ok") is False):
        raise LoginError("operation_unconfirmed")


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
        from curl_cffi import requests, CurlOpt
        import pyotp
        self.request = request
        self.totp = pyotp.TOTP(secret)
        # libcurl also reads proxy/no_proxy environment variables. Pin explicit
        # routes with curl options; trust_env alone does not disable those reads.
        curl_options = ({CurlOpt.NOPROXY: ""} if request.get("proxy") else
                        {CurlOpt.PROXY: ""} if request.get("source_ip") else {})
        self.session = requests.Session(
            impersonate="firefox133", curl_options=curl_options,
            trust_env=not bool(request.get("proxy") or request.get("source_ip")),
        )
        self.proxy = request.get("proxy")
        self.did = str(uuid.uuid4())
        self.sid = str(uuid.uuid4())
        self.selected = None
        self.web_token = None
        self.verified_session = None
        self.stage = "session"
        self.effect_started = False
        self.web_login_performed = False
        self.rejected_navigation = set()
        saved = request.get("web_session")
        if saved and request.get("action") not in ("force_login",):
            if str(saved.get("email", "")).lower() != request["email"].lower():
                raise LoginError("identity_mismatch")
            self.did = str(uuid.UUID(saved["device_id"]))
            if saved.get("session_id"):
                self.sid = str(uuid.UUID(saved["session_id"]))
            for cookie in saved.get("cookies", []):
                if cookie.get("domain", "").lstrip(".") not in ("auth.openai.com", "chatgpt.com", "openai.com"):
                    raise LoginError("invalid_session")
                self.session.cookies.jar.set_cookie(Cookie(
                    0, cookie["name"], cookie["value"], None, False,
                    cookie["domain"], not cookie.get("host_only", False), cookie["domain"].startswith("."),
                    cookie.get("path", "/"), True, cookie.get("secure", True),
                    cookie.get("expires"), cookie.get("expires") is None, None, None, {}, False,
                ))
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
            verify=True, allow_redirects=False, interface=self.request.get("source_ip"), **kwargs,
        )
        if len(response.content) > 2 * 1024 * 1024:
            raise LoginError("response_too_large")
        if response.status_code >= 400:
            path = urlparse(url).path
            if response.status_code == 401 and urlparse(url).path == "/api/auth/session":
                raise LoginError("session_expired")
            if response.status_code == 429:
                raise LoginError("rate_limited")
            body = self.body(response)
            error = body.get("error")
            if (response.status_code == 401 and method == "POST"
                    and urlparse(url).hostname == "chatgpt.com"
                    and path in ("/backend-api/accounts/mfa/user/disable_in_house",
                                 "/backend-api/accounts/mfa/enroll")
                    and body.get("detail") == "User must re-authenticate to enroll/disable a factor"):
                # The security operation was explicitly rejected, while ordinary Web
                # reads may remain valid. Let a fresh login proceed without an uncertain lock.
                self.effect_started = False
                raise LoginError("reauthentication_required")
            if response.status_code in (400, 401, 402, 403):
                code = error.get("code") if isinstance(error, dict) else body.get("code")
                if code in ("account_banned", "account_deactivated", "account_disabled", "account_suspended"):
                    raise LoginError("account_banned")
                if code in ("deactivated_workspace", "workspace_deactivated", "organization_disabled"):
                    raise LoginError("workspace_unavailable")
                if (response.status_code == 401 and urlparse(url).hostname == "chatgpt.com"
                        and path.startswith("/backend-api/accounts/")
                        and code in ("token_revoked", "token_expired")):
                    # /api/auth/session can still return a token after revocation.
                    # Do not preserve that earlier snapshot as a valid Web session.
                    self.verified_session = None
                    self.web_token = None
                    if method == "POST":
                        # This mutation was explicitly rejected before authorization.
                        # A later GET failure must retain any preceding mutation's uncertainty.
                        self.effect_started = False
                    raise LoginError("session_expired")
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
        password_entry_attempted = False
        password_entry_pending = False
        credentials_submitted = False
        password_entry_headers = {
            "Referer": AUTH + "/email-verification",
            "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        }
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
                if password_entry_pending and parsed.netloc != urlparse(AUTH).netloc:
                    raise LoginError("unexpected_redirect")
                if (oauth and parsed.scheme == "http" and parsed.hostname == "localhost"
                        and parsed.port == 1455 and parsed.path == "/auth/callback"
                        and not parsed.username and not parsed.password and not parsed.fragment):
                    query = parse_qs(parsed.query)
                    if query.get("state") != [state] or len(query.get("code", [])) != 1:
                        raise LoginError("callback_mismatch")
                    return query["code"][0]
                response = self.fetch("GET", location,
                                      **({"headers": password_entry_headers} if password_entry_pending else {}))
                continue
            if not oauth and str(response.url).startswith(WEB) and response.status_code == 200:
                return None
            path = urlparse(str(response.url)).path
            if not page_type:
                page_type = {
                    "/log-in": "login_identifier",
                    "/login": "login_identifier",
                    "/log-in/password": "login_password",
                    "/email-verification": "email_otp_verification",
                    "/mfa-challenge": "mfa_challenge",
                    "/sign-in-with-chatgpt/codex/consent": "codex_consent",
                    "/sign-in-with-chatgpt/codex/organization": "codex_organization",
                    "/consent": "consent",
                    "/choose-an-account": "choose_an_account",
                }.get(path, "")
                if path.startswith("/mfa-challenge/"):
                    page_type = "mfa_challenge"
                    data.setdefault("factor_id", path.rsplit("/", 1)[-1])
            if password_entry_pending and page_type == "login_password":
                # The JSON login page is authoritative; an HTML route must contain
                # a password form, not merely have the expected URL.
                html = getattr(response, "text", "")
                mitigation = response.headers.get("cf-mitigated", "") or response.headers.get("x-cf-mitigated", "")
                challenge = ("challenge" in mitigation.lower() or re.search(
                    r"<title[^>]*>\s*(?:just a moment|checking your browser|verify you are human)", html, re.I))
                confirmed = (isinstance(page, dict) and page.get("type") == "login_password") or re.search(
                    r"<input\b[^>]*\btype\s*=\s*(?:\"password\"|'password'|password(?=\s|>))", html, re.I)
                if response.status_code != 200 or challenge or not confirmed:
                    raise LoginError("interaction_required")
                password_entry_pending = False
            elif password_entry_pending and page_type not in ("email_otp_verification", "email_otp_send"):
                raise LoginError("interaction_required")
            counts[page_type] = counts.get(page_type, 0) + 1
            if counts[page_type] > 2:
                raise LoginError("login_loop")
            if page_type in ("login_identifier", "login", "email"):
                endpoint = "/api/accounts/authorize/continue"
                payload = {"username": {"kind": "email", "value": self.request["email"]}}
            elif page_type == "login_password":
                credentials_submitted = True
                endpoint = "/api/accounts/password/verify"
                payload = {"password": self.request["password"]}
            elif page_type == "mfa_challenge":
                credentials_submitted = True
                factor = data.get("factor_id")
                challenge_selected = bool(factor)
                factors = data.get("factors") or data.get("mfa_challenge_factors") or data.get("mfa_factors") or []
                if not factor:
                    factor = next((item.get("id") or item.get("factor_id") for item in factors
                                   if isinstance(item, dict) and (item.get("factor_type") or item.get("type")) == "totp"), None)
                if not factor:
                    raise LoginError("mfa_factor_missing")
                if not challenge_selected:
                    self.fetch("POST", AUTH + "/api/accounts/mfa/issue_challenge", json={
                        "id": factor, "type": "totp", "force_fresh_challenge": False,
                    })
                if time.time() % 30 > 26:
                    time.sleep(30 - time.time() % 30 + 0.1)
                endpoint = "/api/accounts/mfa/verify"
                payload = {"id": factor, "type": "totp", "code": self.totp.now()}
            elif page_type in ("email_otp_verification", "email_otp_send"):
                # Adapt li-zane/sub2api-helper af529bdf's initial Web-login
                # password entry: one navigation in this session, never a retry
                # around an email check imposed after password/TOTP verification.
                if (page_type == "email_otp_verification" and not oauth
                        and not password_entry_attempted and not credentials_submitted
                        and self.request.get("password")):
                    password_entry_attempted = password_entry_pending = True
                    response = self.fetch("GET", AUTH + "/log-in/password",
                                          headers=password_entry_headers)
                    continue
                raise LoginError("email_verification_required")
            elif page_type in ("workspace", "workspace_select", "sign_in_with_chatgpt_codex",
                               "sign_in_with_chatgpt_codex_consent", "codex_consent"):
                choices = data.get("workspaces") or data.get("workspace_choices") or data.get("available_workspaces")
                preferred = self.selected["id"] if self.selected else self.request.get("workspace_id")
                if oauth:
                    if not preferred:
                        raise LoginError("workspace_unknown")
                    workspace = preferred
                else:
                    # Web navigation may use another accessible membership; the final OAuth
                    # selection above always keeps its explicitly requested identity.
                    current = data.get("current_workspace_id") or data.get("workspace_id")
                    candidates = []
                    if isinstance(choices, list):
                        for choice in choices:
                            if not isinstance(choice, dict):
                                continue
                            identifier = choice.get("id") or choice.get("workspace_id")
                            if (identifier and choice.get("can_access") is not False
                                    and choice.get("can_access_with_session") is not False
                                    and not choice.get("is_deactivated") and not choice.get("error")):
                                candidates.append(identifier)
                    elif current:
                        candidates = [current]
                    candidates = [w for w in candidates if w not in self.rejected_navigation]
                    workspace = current if current in candidates else next(iter(candidates), None)
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
            try:
                response = self.fetch(
                    "POST", AUTH + endpoint,
                    **({"json": payload} if payload is not None else {}),
                    headers={"Referer": AUTH + referer},
                )
            except LoginError as error:
                if (not oauth and endpoint == "/api/accounts/workspace/select"
                        and str(error) == "workspace_unavailable"):
                    self.rejected_navigation.add(payload["workspace_id"])
                    counts[page_type] -= 1
                    continue
                raise
        raise LoginError("login_loop")

    def fresh_web_login(self):
        self.web_login_performed = True
        self.stage = "web_login"
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
        self.verify_web_session()

    def verify_web_session(self):
        response = self.fetch("GET", WEB + "/api/auth/session")
        try:
            data = response.json()
        except (ValueError, TypeError):
            raise LoginError("session_response_invalid") from None
        if response.status_code != 200 or not isinstance(data, dict):
            raise LoginError("session_response_invalid")
        token = data.get("accessToken") or data.get("access_token")
        if not token:
            raise LoginError("session_expired" if not data else "session_response_invalid")
        email = (data.get("user") or {}).get("email")
        if not isinstance(email, str) or email.lower() != self.request["email"].lower():
            raise LoginError("identity_mismatch")
        self.web_token = token
        # Capture the jar at verification time. Later OAuth/security responses may change
        # cookies and must not be mislabeled as a newly verified session.
        cookies = []
        for cookie in self.session.cookies.jar:
            if cookie.domain.lstrip(".") in ("auth.openai.com", "chatgpt.com", "openai.com"):
                cookies.append({"name": cookie.name, "value": cookie.value, "domain": cookie.domain,
                                "path": cookie.path, "secure": cookie.secure, "expires": cookie.expires,
                                "host_only": not cookie.domain_specified})
        self.verified_session = {"email": self.request["email"], "device_id": self.did, "cookies": cookies,
                                 "session_id": self.sid,
                                 "verified_at": datetime.now(timezone.utc).isoformat()}

    def establish_web(self, allow_login=False):
        if self.request.get("web_session") and self.request.get("action") != "force_login":
            try:
                self.verify_web_session()
                return
            except LoginError as error:
                if str(error) != "session_expired" or not allow_login:
                    raise
                # A rejected session must not contaminate the new login jar.
                self.session.cookies.clear()
                self.verified_session = None
        elif not allow_login:
            raise LoginError("session_missing")
        self.fresh_web_login()

    def web_snapshot(self):
        return self.verified_session

    def web_api(self, method, path, payload=None):
        headers = {"Authorization": "Bearer " + self.web_token, "oai-session-id": self.sid,
                   "oai-language": "zh-CN", "x-openai-target-path": "/backend-api" + path,
                   "x-openai-target-route": "/backend-api" + path}
        return self.fetch(method, WEB + "/backend-api" + path, headers=headers,
                          **({"json": payload} if payload is not None else {}))

    def memberships(self):
        self.stage = "workspace_directory"
        return self.body(self.web_api("GET", "/accounts/check/v4-2023-04-27")).get("accounts")

    def authorize(self):
        try:
            accounts = self.memberships()
        except LoginError as error:
            if (str(error) != "session_expired" or not self.request.get("web_session")
                    or self.web_login_performed or self.request.get("action") == "force_login"):
                raise
            # Only a definitively expired saved Web session permits one fresh login.
            # Keep the requested workspace and stop if its replacement also fails.
            self.session.cookies.clear()
            self.verified_session = None
            self.fresh_web_login()
            accounts = self.memberships()
        self.selected = select_workspace(accounts, self.request.get("workspace_id"))
        self.stage = "oauth_authorize"
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
        self.stage = "token_exchange"
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
        self.stage = "credential_verification"
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
        return {"ok": True, "document": document, "web_session": self.web_snapshot()}


    def devices(self):
        data = self.body(self.web_api("GET", "/accounts/sessions?include_trusted_devices=true"))
        if not isinstance(data.get("devices"), list):
            raise LoginError("devices_unavailable")
        result = []
        for item in data["devices"][:500]:
            if not isinstance(item, dict):
                continue
            session_id, device_hash = item.get("session_id"), item.get("hashed_device_id")
            if not session_id and not device_hash:
                continue
            result.append({"id": str(item.get("render_id") or session_id or device_hash)[:256],
                           "sessionId": session_id, "deviceIdHash": device_hash,
                           "name": str(item.get("display_name") or "未知设备")[:256],
                           "platform": str(item.get("platform") or "")[:128],
                           "current": item.get("is_current_device") is True,
                           "trusted": item.get("is_trusted_device") is True,
                           "lastActiveAt": item.get("last_signed_in_timestamp_second")})
        return result

    def security(self):
        info = self.body(self.web_api("GET", "/accounts/mfa_info"))
        totp = totp_state(info)
        password = self.body(self.web_api("GET", "/accounts/change_password/eligibility")).get("eligible")
        if password is False:
            add = self.body(self.web_api("GET", "/accounts/add_password/eligibility")).get("eligible")
            password = False if add is True else None
        return info, {"totpEnabled": totp if isinstance(totp, bool) else None,
                      "passwordEnabled": password if isinstance(password, bool) else None}

    def run(self):
        action = self.request.get("action", "authorize")
        self.establish_web(allow_login=action in ("authorize", "force_login", "switch_workspace"))
        if action in ("authorize", "force_login", "switch_workspace"):
            return self.authorize()
        self.stage = action
        view = {"sessionState": "valid", "sessionCheckedAt": datetime.now(timezone.utc).isoformat()}
        result = {"ok": True, "view": view}
        if action in ("check_session", "workspaces"):
            view["workspaces"] = [{"id": w["id"], "name": w["name"], "planType": w["plan"],
                                   "available": w["available"]} for w in workspace_directory(self.memberships())]
        elif action == "devices":
            view["devices"] = self.devices()
        elif action == "security":
            _, security = self.security()
            view.update(security)
        elif action == "revoke_device":
            target, device_hash = self.request.get("target"), self.request.get("device_id_hash")
            if bool(target) == bool(device_hash):
                raise LoginError("device_missing")
            device = next((d for d in self.devices() if (d["sessionId"] == target if target else d["deviceIdHash"] == device_hash)), None)
            if not device or device["current"] or (device_hash and not device["trusted"]):
                raise LoginError("device_missing")
            self.effect_started = True
            acknowledged(self.web_api("POST", "/accounts/trusted_devices/revoke" if device_hash else "/accounts/sessions/revoke",
                         {"device_id_hash": device_hash} if device_hash else {"session_id": target}))
            view["message"] = "退出请求已受理，刷新设备列表确认"
        elif action in ("logout_all", "logout_and_relogin"):
            self.effect_started = True
            acknowledged(self.web_api("POST", "/accounts/logout_all", {}), allow_null=True)
            self.verified_session = None
            view["sessionState"] = "absent"
            view["message"] = "全部退出请求已受理，上游可能延迟生效"
            # Combined recovery is two durable service stages, never one opaque subprocess.
        elif action == "prepare_totp":
            info, security = self.security()
            if security["totpEnabled"] is not False:
                raise LoginError("totp_already_enabled")
            self.effect_started = True
            enrollment = self.body(self.web_api("POST", "/accounts/mfa/enroll", {"factor_type": "totp"}))
            secret, session_id = enrollment.get("secret"), enrollment.get("session_id")
            if not isinstance(secret, str) or not isinstance(session_id, str):
                raise LoginError("totp_enroll_invalid")
            secret = "".join(secret.upper().split()).rstrip("=")
            if not 16 <= len(secret) <= 128 or any(c not in "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567" for c in secret):
                raise LoginError("totp_enroll_invalid")
            result["pending_totp"] = {"secret": secret, "session_id": session_id,
                                      "created_at": datetime.now(timezone.utc).isoformat()}
            view["pendingTotp"] = True
        elif action == "activate_totp":
            import pyotp
            pending = self.request.get("pending_totp")
            if not pending:
                raise LoginError("totp_enroll_missing")
            if time.time() % 30 > 26:
                time.sleep(30 - time.time() % 30 + .1)
            self.effect_started = True
            activated = self.body(self.web_api("POST", "/accounts/mfa/user/activate_enrollment", {
                "code": pyotp.TOTP(pending["secret"]).now(), "factor_type": "totp", "session_id": pending["session_id"],
            }))
            if activated.get("success") is not True:
                raise LoginError("totp_activation_unconfirmed")
            # Acknowledged activation is proof for this specific key; a generic enabled flag is not.
            result["new_secret"] = pending["secret"]
            view["totpEnabled"] = True
            view["pendingTotp"] = False
        elif action == "disable_totp":
            info, security = self.security()
            factor_ids = list(totp_factor_ids(info))
            if len(factor_ids) != 1 or not factor_ids[0]:
                raise LoginError("mfa_factor_missing")
            self.effect_started = True
            response = self.web_api("POST", "/accounts/mfa/user/disable_in_house", {"factor_id": factor_ids[0]})
            # The endpoint also acknowledges with message-only JSON. The subsequent
            # state read, not a particular acknowledgement field, proves disablement.
            acknowledged(response)
            _, security = self.security()
            if security["totpEnabled"] is not False:
                raise LoginError("totp_disable_unconfirmed")
            view.update(security)
        else:
            raise LoginError("unsupported_action")
        result["web_session"] = self.web_snapshot()
        return result


def main():
    login = None
    try:
        request = json.loads(sys.stdin.buffer.read(512 * 1024 + 1))
        login = Login(request)
        result = login.run()
    except ImportError:
        result = {"ok": False, "code": "runtime_missing"}
    except WorkspaceSelectionRequired as error:
        result = {"ok": False, "code": "workspace_ambiguous", "workspaces": error.choices}
    except LoginError as error:
        result = {"ok": False, "code": str(error), "stage": login.stage if login else "initialization",
                  "uncertain": bool(login and login.effect_started)}
    except Exception:
        # Exception text may contain credentials, cookies or upstream bodies.
        result = {"ok": False, "code": "transport_failed", "stage": login.stage if login else "initialization",
                  "uncertain": bool(login and login.effect_started)}
    finally:
        if login:
            if login.web_snapshot() and "web_session" not in result:
                result["web_session"] = login.web_snapshot()
            login.session.close()
    print(json.dumps(result, separators=(",", ":")), flush=True)


if __name__ == "__main__":
    main()
