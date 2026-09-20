"""Bounded container egress checks; never mutate managed proxy/account state."""

import concurrent.futures
import ipaddress
import json
import math
import re
import subprocess
from urllib.parse import urlsplit


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


def verify(profile, app, protected):
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
