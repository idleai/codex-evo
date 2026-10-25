import json
import os
from unittest import mock
import unittest

from check_ci_results import main


class ResultTests(unittest.TestCase):
    def run_gate(self, results, optional=()):
        environment = {
            "NEEDS": json.dumps(
                {name: {"result": value} for name, value in results.items()}
            ),
            "OPTIONAL_SKIPPED": json.dumps(optional),
        }
        with mock.patch.dict(os.environ, environment):
            main()

    def test_success(self):
        self.run_gate({"preflight": "success", "linux": "success"})

    def test_explicitly_unselected_job(self):
        self.run_gate({"preflight": "success", "linux": "skipped"}, ["linux"])

    def test_failed_preflight_cannot_pass(self):
        with self.assertRaises(SystemExit):
            self.run_gate({"preflight": "failure", "linux": "skipped"}, ["linux"])

    def test_unexpected_skip_cannot_pass(self):
        with self.assertRaises(SystemExit):
            self.run_gate({"preflight": "success", "linux": "skipped"})

    def test_optional_failure_and_cancellation_cannot_pass(self):
        for result in ("failure", "cancelled"):
            with self.subTest(result=result), self.assertRaises(SystemExit):
                self.run_gate({"preflight": "success", "linux": result}, ["linux"])

    def test_unknown_optional_job_is_rejected(self):
        with self.assertRaises(ValueError):
            self.run_gate({"preflight": "success"}, ["missing"])


if __name__ == "__main__":
    unittest.main()
