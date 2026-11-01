from copy import deepcopy
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

from report_async_failure import report


class FailureTests(unittest.TestCase):
    def setUp(self):
        self.run = {
            "id": 42,
            "workflow_id": 7,
            "head_sha": "a" * 40,
            "name": "Release",
            "html_url": "https://github.com/idleai/test/actions/runs/42",
            "run_attempt": 1,
            "status": "completed",
            "conclusion": "failure",
            "event": "push",
            "head_branch": "main",
            "head_repository": {"full_name": "idleai/test"},
        }
        self.issues = []
        self.writes = []

    def api(self, resource, *, method="GET", body=None):
        if method != "GET":
            self.writes.append((method, resource, deepcopy(body)))
            issue = dict(
                body,
                number=8,
                html_url="https://github.com/idleai/test/issues/8",
                state=body.get("state", "open"),
            )
            self.issues[:] = [issue]
            return issue
        if resource == "repos/idleai/test/actions/runs/42":
            return self.run
        if resource == "repos/idleai/test":
            return {"default_branch": "main"}
        if "/jobs?" in resource:
            return {
                "jobs": [
                    {
                        "name": "Linux",
                        "conclusion": "failure",
                        "html_url": self.run["html_url"] + "/job/1",
                        "steps": [{"name": "Full tests", "conclusion": "failure"}],
                    }
                ]
            }
        if "/issues?" in resource:
            return self.issues
        if "/git/ref/tags/" in resource:
            if resource.endswith("rust-v1.0.0"):
                return {"ref": "refs/tags/rust-v1.0.0"}
            raise HTTPError(resource, 404, "Not Found", {}, None)
        raise AssertionError(resource)

    def test_failed_candidate_creates_an_issue_and_retry_updates_it(self):
        with patch("report_async_failure.api", side_effect=self.api):
            report("idleai/test", 42)
            self.assertEqual(self.writes[0][0], "POST")
            self.assertIn("Full tests", self.issues[0]["body"])
            report("idleai/test", 42)
            self.assertEqual(len(self.writes), 1)
            self.issues[0]["state"] = "closed"
            self.run["run_attempt"] = 2
            report("idleai/test", 42)
            self.assertEqual(self.writes[-1][0], "PATCH")
            self.assertEqual(self.issues[0]["state"], "open")
            self.assertIn("Run attempt: 2", self.issues[0]["body"])

    def test_pr_foreign_cancelled_and_successful_retry_runs_do_not_create_issues(self):
        for change in [
            {"event": "pull_request"},
            {"head_repository": {"full_name": "other/fork"}},
            {"conclusion": "cancelled"},
            {"conclusion": "success"},
            {"status": "in_progress"},
            {"head_branch": "candidate", "event": "workflow_dispatch"},
        ]:
            with (
                self.subTest(change=change),
                patch("report_async_failure.api", side_effect=self.api),
            ):
                original = self.run.copy()
                self.run.update(change)
                report("idleai/test", 42)
                self.assertEqual(self.writes, [])
                self.run = original

    def test_dry_run_reads_real_run_details_without_writing_an_issue(self):
        with patch("report_async_failure.api", side_effect=self.api):
            report("idleai/test", 42, dry_run=True)
        self.assertEqual(self.writes, [])

    def test_release_tag_is_reported_but_an_unmerged_branch_is_not(self):
        with patch("report_async_failure.api", side_effect=self.api):
            self.run["head_branch"] = "unmerged-work"
            report("idleai/test", 42)
            self.assertEqual(self.writes, [])
            self.run["head_branch"] = "rust-v1.0.0"
            report("idleai/test", 42)
            self.assertEqual(self.writes[0][0], "POST")
