import copy
import json
from pathlib import Path
import secrets
import subprocess
import sys
import unittest
from unittest.mock import patch
from urllib.parse import quote, urlunsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import egress_check
import migration_backup


def check():
    return {"name": "direct-v4", "url": "https://probe.example.test/", "family": "ipv4"}


def response(stdout="200|192.0.2.5|0.125", code=0, stderr=""):
    return subprocess.CompletedProcess([], code, stdout=stdout, stderr=stderr)


def synthetic_proxy(scheme="http", port=8080):
    password = quote(secrets.token_urlsafe(18), safe="")
    authority = f"synthetic-user:{password}@proxy.example.test"
    if port is not None:
        authority += f":{port}"
    return urlunsplit((scheme, authority, "/", "", ""))


class EgressCheckTests(unittest.TestCase):
    def test_validate_optional_and_bounded_checks(self):
        self.assertEqual(egress_check.validate_checks([]), [])
        self.assertEqual(egress_check.validate_checks([check()]), [check()])
        invalid = [
            None, {}, [check()] * 9, [check(), check()],
            [{**check(), "url": "http://probe.example.test/"}],
            [{**check(), "url": synthetic_proxy("https")}],
            [{**check(), "url": "https://probe.example.test/\nnext"}],
            [{**check(), "family": "other"}],
            [{**check(), "name": "contains secret"}],
            [{**check(), "proxy_id": "bad'identity"}],
            [{**check(), "proxy_url": synthetic_proxy()}],
        ]
        for value in invalid:
            with self.subTest(value=value), self.assertRaises(ValueError):
                egress_check.validate_checks(value)

    def test_disabled_checks_do_not_touch_docker_or_database(self):
        with patch.object(egress_check.subprocess, "run") as run:
            self.assertEqual(egress_check.verify({}, {}, {}), [])
        run.assert_not_called()

    def test_direct_ipv4_is_bounded_and_ignores_environment_proxy(self):
        with patch.object(egress_check.subprocess, "run", return_value=response()) as run:
            result = egress_check.probe("example-app", check())
        args = run.call_args.args[0]
        self.assertIn("-4", args)
        self.assertIn("--max-time", args)
        self.assertEqual(run.call_args.kwargs["timeout"], 17)
        self.assertIn('noproxy = "*"', run.call_args.kwargs["input"])
        self.assertIn('proxy = ""', run.call_args.kwargs["input"])
        self.assertEqual(result, {"name": "direct-v4", "http_status": 200,
                                 "peer_family": "ipv4", "via_proxy": False, "seconds": 0.125})
        self.assertNotIn("192.0.2.5", json.dumps(result))

    def test_ipv6_and_automatic_family(self):
        for family in ["ipv6", "auto"]:
            item = {**check(), "family": family}
            with patch.object(egress_check.subprocess, "run",
                              return_value=response("204|2001:db8::2|0.05")) as run:
                result = egress_check.probe("example-app", item)
            self.assertEqual(result["peer_family"], "ipv6")
            self.assertEqual("-6" in run.call_args.args[0], family == "ipv6")

    def test_failure_never_discloses_upstream_or_proxy_credentials(self):
        failures = [
            response("000||0.002", 7, "secret in diagnostic"),
            response("403|192.0.2.5|0.1", 22),
            response("200|2001:db8::2|0.1"),
            response("200|192.0.2.5|nan"),
            response("unexpected body"),
        ]
        for failure in failures:
            with self.subTest(failure=failure.returncode), \
                    patch.object(egress_check.subprocess, "run", return_value=failure), \
                    self.assertRaisesRegex(RuntimeError, "^Egress check failed: direct-v4$"):
                egress_check.probe("example-app", check(), synthetic_proxy())
        with patch.object(egress_check.subprocess, "run",
                          side_effect=subprocess.TimeoutExpired(["curl"], 17, stderr="secret")), \
                self.assertRaisesRegex(RuntimeError, "^Egress check failed: direct-v4$"):
            egress_check.probe("example-app", check())

    def test_proxy_secret_is_stdin_only_and_bypasses_no_proxy_env(self):
        secret = synthetic_proxy()
        with patch.object(egress_check.subprocess, "run", return_value=response()) as run:
            result = egress_check.probe("example-app", check(), secret)
        self.assertNotIn(secret, repr(run.call_args.args))
        self.assertIn(secret, run.call_args.kwargs["input"])
        self.assertIn('noproxy = ""', run.call_args.kwargs["input"])
        self.assertNotIn(secret, repr(result))

    def test_managed_proxy_lookup_is_read_only_and_not_shell_interpolated(self):
        secret = synthetic_proxy(port=None)
        with patch.object(migration_backup, "database_container", return_value="example-db"), \
                patch.object(egress_check.subprocess, "run",
                             return_value=response(secret + "\n")) as run:
            self.assertEqual(egress_check.managed_proxy_url("proxy_example", {}, {}), secret)
        args = run.call_args.args[0]
        self.assertIn("PGOPTIONS=-c default_transaction_read_only=on", args)
        self.assertIn("proxy_id=proxy_example", args)
        self.assertIn("where id = :'proxy_id'", run.call_args.kwargs["input"])
        self.assertEqual(run.call_args.kwargs["timeout"], 15)
        self.assertNotIn(secret, repr(run.call_args))

    def test_missing_or_invalid_managed_proxy_is_rejected(self):
        for value, code in [("", 0), ("http://a.example.test/\nhttp://b.example.test/", 0),
                            ("file:///etc/hosts", 0), ("http://proxy.example.test:invalid", 0),
                            ("http://proxy.example.test:8080", 1)]:
            with patch.object(migration_backup, "database_container", return_value="example-db"), \
                    patch.object(egress_check.subprocess, "run", return_value=response(value, code)), \
                    self.assertRaisesRegex(RuntimeError, "missing or invalid"):
                egress_check.managed_proxy_url("proxy_example", {}, {})

    def test_proxy_is_resolved_once_for_parallel_checks(self):
        items = [check(), {**check(), "name": "proxy-v4", "proxy_id": "proxy_example"},
                 {**check(), "name": "proxy-auto", "family": "auto", "proxy_id": "proxy_example"}]
        original = copy.deepcopy(items)
        with patch.object(egress_check, "managed_proxy_url", return_value="http://proxy.example.test") as lookup, \
                patch.object(egress_check, "probe", side_effect=lambda _, item, __: {"name": item["name"]}):
            result = egress_check.verify({"container": "example-app", "egress_checks": items}, {}, {})
        lookup.assert_called_once_with("proxy_example", {}, {})
        self.assertEqual([item["name"] for item in result], [item["name"] for item in items])
        self.assertEqual(items, original)


if __name__ == "__main__":
    unittest.main()
