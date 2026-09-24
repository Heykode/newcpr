"""Offline workspace-selection fixtures. No network or live account material."""
import io
import json
import types
import unittest
from unittest.mock import Mock, patch

from relogin_worker_test import worker


class WorkspaceSelectionTests(unittest.TestCase):
    def test_ambiguity_returns_only_deduplicated_highest_choices(self):
        accounts = [
            {"id": "personal", "plan_type": "free"},
            {"id": "business-b", "name": "Second Team", "plan_type": "business"},
            {"id": "business-a", "name": " First\nTeam ", "plan_type": "team",
             "access_token": "never-output-token"},
            {"id": "business-a", "plan_type": "self_serve_business_prolite"},
        ]
        with self.assertRaises(worker.WorkspaceSelectionRequired) as caught:
            worker.select_workspace(accounts)
        self.assertEqual(caught.exception.choices, [
            {"id": "business-a", "name": "FirstTeam", "planType": "business"},
            {"id": "business-b", "name": "Second Team", "planType": "business"},
        ])
        self.assertNotIn("never-output", json.dumps(caught.exception.choices))
        self.assertEqual(worker.select_workspace(accounts, "business-b"),
                         {"id": "business-b", "plan": "business"})

    def test_ambiguity_output_is_bounded_and_unknown_plans_still_stop(self):
        accounts = [{"id": str(i), "name": "A" * 300, "plan_type": "team"} for i in range(2)]
        with self.assertRaises(worker.WorkspaceSelectionRequired) as caught:
            worker.select_workspace(accounts)
        self.assertTrue(all(len(choice["name"]) == 128 for choice in caught.exception.choices))
        for entries in [
            [{"id": str(i), "plan_type": "team"} for i in range(65)],
            [{"id": "bad\nid", "plan_type": "team"}, {"id": "other", "plan_type": "team"}],
            accounts + [{"id": "unknown", "plan_type": "future"}],
        ]:
            with self.assertRaises(worker.LoginError) as error:
                worker.select_workspace(entries)
            self.assertNotIsInstance(error.exception, worker.WorkspaceSelectionRequired)

    def test_main_returns_choices_and_closes_session_without_exporting_secrets(self):
        choices = [{"id": name, "name": name, "planType": "business"} for name in ["a", "b"]]
        instance = Mock()
        instance.run.side_effect = worker.WorkspaceSelectionRequired(choices)
        output = io.StringIO()
        with patch.object(worker, "Login", return_value=instance), \
                patch.object(worker.sys, "stdin", types.SimpleNamespace(buffer=io.BytesIO(b"{}"))), \
                patch.object(worker.sys, "stdout", output):
            worker.main()
        self.assertEqual(json.loads(output.getvalue()),
                         {"ok": False, "code": "workspace_ambiguous", "workspaces": choices})
        instance.session.close.assert_called_once()


if __name__ == "__main__":
    unittest.main()
