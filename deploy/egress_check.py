"""Bounded container egress checks; never mutate managed proxy/account state."""

import concurrent.futures
import copy
import ipaddress
import json
import math
import re
import subprocess
from urllib.parse import urlsplit


IPV6_POLICY_LABEL = "io.newcpr.ipv6-disabled"
IPV6_SYSCTLS = ("net.ipv6.conf.all.disable_ipv6", "net.ipv6.conf.default.disable_ipv6")


def validate_network_policy(profile):
    policy = profile.get("ipv6_policy")
    if policy not in (None, "disabled"):
        raise ValueError("ipv6_policy must be disabled or omitted")
    if policy == "disabled" and any(
            item.get("family") == "ipv6" for item in profile.get("egress_checks", [])):
        raise ValueError("Disabled IPv6 conflicts with an IPv6 egress check")
    return policy == "disabled"


def network_details(names):
    if not names:
        raise RuntimeError("Application must retain an isolated network")
    try:
        result = subprocess.run(["docker", "network", "inspect", *names],
                                capture_output=True, text=True, timeout=15, check=True)
        networks = json.loads(result.stdout)
        if len(networks) != len(names):
            raise ValueError("Missing network")
        return {item["Name"]: item for item in networks}
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError):
        raise RuntimeError("Cannot verify application network policy") from None


def policy_required(profile, app):
    required = validate_network_policy(profile)
    labels = app.get("Config", {}).get("Labels") or {}
    if labels.get(IPV6_POLICY_LABEL) == "true" and not required:
        raise RuntimeError("Existing IPv6-disabled policy must remain in the deployment profile")
    return required


def verify_network_policy(profile, app, *, transition=False):
    if not policy_required(profile, app):
        return
    host = app.get("HostConfig", {})
    if host.get("NetworkMode") in ("host", "none") or str(host.get("NetworkMode", "")).startswith("container:"):
        raise RuntimeError("IPv6 policy requires an isolated application network namespace")
    if transition:
        return
    sysctls = host.get("Sysctls") or {}
    if any(str(sysctls.get(key)) != "1" for key in IPV6_SYSCTLS):
        raise RuntimeError("Application lacks persistent IPv6 disable settings")
    networks = network_details(list(app.get("NetworkSettings", {}).get("Networks", {})))
    if any(item.get("EnableIPv6") for item in networks.values()):
        raise RuntimeError("Application has an IPv6 network attached")
    try:
        subprocess.run([
            "docker", "exec", profile["container"], "sh", "-ec",
            'test "$(cat /proc/sys/net/ipv6/conf/all/disable_ipv6)" = 1; '
            'test "$(cat /proc/sys/net/ipv6/conf/default/disable_ipv6)" = 1; '
            'test -z "$(cat /proc/net/if_inet6)"',
        ], capture_output=True, timeout=15, check=True)
    except (OSError, subprocess.SubprocessError):
        raise RuntimeError("Application IPv6 is not disabled at runtime") from None


def mapping(value):
    if isinstance(value, list):
        return {item.partition("=")[0]: item.partition("=")[2] for item in value}
    return dict(value or {})


def ipv6_disabled_document(document, rendered, service, *, transition=False):
    result = copy.deepcopy(document)
    app = result["services"][service]
    resolved = rendered["services"][service]
    if resolved.get("network_mode"):
        raise RuntimeError("IPv6 policy requires explicit isolated Compose networks")
    attached = resolved.get("networks") or {}
    names = {key: rendered["networks"][key]["name"] for key in attached}
    networks = network_details(list(names.values()))
    ipv6_keys = {
        key for key, name in names.items()
        if networks[name].get("EnableIPv6")
        or rendered["networks"][key].get("enable_ipv6")
        or (attached[key] or {}).get("ipv6_address")
    }
    if ipv6_keys and not transition:
        raise RuntimeError("Candidate Compose configuration would reconnect IPv6")
    if any(networks[name].get("Driver") in ("host", "null") for name in names.values()):
        raise RuntimeError("IPv6 policy requires isolated Compose networks")
    if transition:
        configured = app.get("networks", list(attached))
        if isinstance(configured, list):
            app["networks"] = [key for key in configured if key not in ipv6_keys]
        else:
            app["networks"] = {key: value for key, value in configured.items() if key not in ipv6_keys}
        if not app["networks"]:
            raise RuntimeError("Disabling IPv6 would remove every application network")
        app["sysctls"] = {**mapping(app.get("sysctls")), **dict.fromkeys(IPV6_SYSCTLS, "1")}
        app["labels"] = {**mapping(app.get("labels")), IPV6_POLICY_LABEL: "true"}
    else:
        if any(str(mapping(resolved.get("sysctls")).get(key)) != "1" for key in IPV6_SYSCTLS):
            raise RuntimeError("Candidate Compose configuration would enable IPv6")
        if mapping(resolved.get("labels")).get(IPV6_POLICY_LABEL) != "true":
            raise RuntimeError("Candidate Compose configuration removed the IPv6 policy marker")
    return result


def validate_checks(checks):
    if not isinstance(checks, list) or len(checks) > 8:
        raise ValueError("egress_checks must contain at most eight checks")
    names = set()
    for check in checks:
        if not isinstance(check, dict) or set(check) - {"name", "url", "family", "proxy_id"}:
            raise ValueError("Invalid egress check fields")
        name = check.get("name")
        if not isinstance(name, str) or not re.fullmatch(r"[a-z][a-z0-9_-]{0,31}", name) or name in names:
            raise ValueError("Egress check names must be unique safe identifiers")
        names.add(name)
        url = check.get("url")
        if not isinstance(url, str) or len(url) > 2048 or any(ord(char) < 32 for char in url):
            raise ValueError("Invalid egress check URL")
        endpoint = urlsplit(url)
        if (endpoint.scheme != "https" or not endpoint.hostname
                or endpoint.username is not None or endpoint.password is not None or endpoint.fragment):
            raise ValueError("Egress checks require unauthenticated HTTPS targets")
        if check.get("family", "auto") not in {"auto", "ipv4", "ipv6"}:
            raise ValueError("Invalid egress address family")
        if "proxy_id" in check and (
                not isinstance(check["proxy_id"], str)
                or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", check["proxy_id"])):
            raise ValueError("Invalid managed proxy identity")
    return checks


def managed_proxy_url(proxy_id, app, protected):
    from migration_backup import database_container

    database = database_container(app, protected)
    try:
        result = subprocess.run(
            ["docker", "exec", "-i", "-e", "PGOPTIONS=-c default_transaction_read_only=on",
             database, "sh", "-ec",
             'PGPASSWORD="$POSTGRES_PASSWORD" psql -X -w -h 127.0.0.1 '
             '-U "$POSTGRES_USER" -d "$POSTGRES_DB" -At -v ON_ERROR_STOP=1 "$@"',
             "psql", "-v", "proxy_id=" + proxy_id],
            input="select proxy_url from public.outbound_proxies where id = :'proxy_id';\n",
            text=True, capture_output=True, timeout=15,
        )
    except (OSError, subprocess.SubprocessError):
        raise RuntimeError("Cannot read the managed egress-check proxy") from None
    value = result.stdout.rstrip("\n")
    try:
        endpoint = urlsplit(value)
        valid = (endpoint.scheme in {"http", "https", "socks5", "socks5h"}
                 and endpoint.hostname and (endpoint.port is None or endpoint.port > 0))
    except ValueError:
        valid = False
    if (result.returncode or not value or len(value) > 4096
            or any(ord(char) < 32 for char in value) or not valid):
        raise RuntimeError("Managed egress-check proxy is missing or invalid")
    return value


def probe(container, check, proxy_url=None):
    name = check["name"]
    # Feed all URLs through stdin: proxy credentials must not enter argv or logs.
    config = [
        "url = " + json.dumps(check["url"], ensure_ascii=False),
        "proxy = " + json.dumps(proxy_url or "", ensure_ascii=False),
        "noproxy = " + json.dumps("" if proxy_url else "*"),
    ]
    args = [
        "docker", "exec", "-i", container, "curl", "-q", "--config", "-",
        "--silent", "--show-error", "--fail", "--connect-timeout", "5",
        "--max-time", "12", "--output", "/dev/null",
        "--write-out", "%{http_code}|%{remote_ip}|%{time_total}",
    ]
    family = check.get("family", "auto")
    if family != "auto":
        args.append("-4" if family == "ipv4" else "-6")
    try:
        result = subprocess.run(
            args, input="\n".join(config) + "\n", text=True, capture_output=True, timeout=17,
        )
        status, remote, elapsed = result.stdout.strip().split("|")
        peer_family = "ipv" + str(ipaddress.ip_address(remote).version)
        seconds = float(elapsed)
        valid = (result.returncode == 0 and 200 <= int(status) < 300
                 and (family == "auto" or peer_family == family)
                 and math.isfinite(seconds) and 0 <= seconds <= 17)
    except (OSError, subprocess.SubprocessError, ValueError):
        valid = False
    if not valid:
        raise RuntimeError(f"Egress check failed: {name}") from None
    return {"name": name, "http_status": int(status), "peer_family": peer_family,
            "via_proxy": proxy_url is not None, "seconds": round(seconds, 3)}


def verify(profile, app, protected, *, ipv6_transition=False):
    verify_network_policy(profile, app, transition=ipv6_transition)
    checks = validate_checks(profile.get("egress_checks", []))
    if not checks:
        return []
    proxies = {
        key: managed_proxy_url(key, app, protected)
        for key in {check["proxy_id"] for check in checks if "proxy_id" in check}
    }
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(checks)) as executor:
        futures = [
            executor.submit(probe, profile["container"], check, proxies.get(check.get("proxy_id")))
            for check in checks
        ]
        return [future.result() for future in futures]
