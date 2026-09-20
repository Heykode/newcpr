"""Opt-in Linux Docker test; all network resources belong to a random test project."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import uuid

import yaml


@unittest.skipUnless(os.environ.get("CPR_EGRESS_DOCKER_TEST") == "1", "explicit network test opt-in required")
class DockerEgressTests(unittest.TestCase):
    def test_gateway_priority_survives_application_recreation(self):
        suffix = uuid.uuid4().hex[:10]
        name = "cpr-egress-smoke-" + suffix
        parent = "cpre" + suffix
        macvlan = name + "-mac"
        root = [] if os.geteuid() == 0 else ["sudo", "-n"]
        image = os.environ.get("CPR_EGRESS_TEST_IMAGE", "node:24-bookworm-slim")

        def command(*args):
            return subprocess.check_output(args, stderr=subprocess.STDOUT, text=True, timeout=45).strip()

        def inspect():
            return json.loads(command("docker", "inspect", name + "-app"))[0]

        def route_counts():
            pid = str(inspect()["State"]["Pid"])
            return {
                family: len(json.loads(command(*root, "nsenter", "-t", pid, "-n",
                                               "ip", "-j", "-" + family, "route", "show", "default")))
                for family in ("4", "6")
            }

        with tempfile.TemporaryDirectory(prefix=name) as temporary:
            path = Path(temporary) / "compose.yaml"
            document = {
                "name": name,
                "services": {"app": {
                    "image": image, "container_name": name + "-app",
                    "entrypoint": ["sleep", "300"], "stop_grace_period": "1s",
                    "networks": {"v4": {"gw_priority": 0}, "v6": {"gw_priority": 0}},
                }},
                "networks": {"v4": {}, "v6": {"external": True, "name": macvlan}},
            }
            path.write_text(yaml.safe_dump(document))
            link_created = False
            network_created = False
            try:
                command(*root, "ip", "link", "add", parent, "type", "dummy")
                link_created = True
                command(*root, "ip", "link", "set", parent, "up")
                command("docker", "network", "create", "-d", "macvlan", "--ipv6",
                        "--subnet", f"2001:db8:{suffix[:4]}::/64",
                        "--gateway", f"2001:db8:{suffix[:4]}::1",
                        "-o", "parent=" + parent, macvlan)
                network_created = True
                command("docker", "compose", "-f", str(path), "up", "-d", "--pull", "never")
                baseline = route_counts()
                self.assertEqual(baseline["6"], 1)
                document["services"]["app"]["networks"]["v4"]["gw_priority"] = 1
                path.write_text(yaml.safe_dump(document))
                command("docker", "compose", "-f", str(path), "up", "-d", "--pull", "never")
                self.assertEqual(route_counts(), {"4": 1, "6": 1})
                first = inspect()["Id"]
                command("docker", "compose", "-f", str(path), "up", "-d", "--pull", "never",
                        "--force-recreate", "app")
                self.assertNotEqual(inspect()["Id"], first)
                self.assertEqual(route_counts(), {"4": 1, "6": 1})
                print(json.dumps({"case": "gateway-priority-recreation", "baseline_routes": baseline,
                                  "after_recreation": route_counts()}), flush=True)
            finally:
                subprocess.run(["docker", "compose", "-f", str(path), "down"],
                               capture_output=True, timeout=45, check=True)
                if network_created:
                    command("docker", "network", "rm", macvlan)
                if link_created:
                    command(*root, "ip", "link", "del", parent)


if __name__ == "__main__":
    unittest.main()
