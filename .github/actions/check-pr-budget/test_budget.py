from datetime import datetime, timezone
from io import StringIO
import os
import unittest
from unittest.mock import patch

from budget import elapsed_seconds, main


class BudgetTests(unittest.TestCase):
    def test_current_attempt_includes_waiting_and_does_not_charge_previous_attempts(
        self,
    ):
        run = {
            "created_at": "2026-01-01T00:00:00Z",
            "run_started_at": "2026-01-02T00:00:00Z",
        }
        self.assertEqual(
            elapsed_seconds(run, datetime(2026, 1, 2, 0, 9, 59, tzinfo=timezone.utc)),
            599,
        )
        self.assertEqual(
            elapsed_seconds(run, datetime(2026, 1, 2, 0, 10, 1, tzinfo=timezone.utc)),
            601,
        )

    def test_invalid_clock_fails_instead_of_reporting_a_short_run(self):
        with self.assertRaises(ValueError):
            elapsed_seconds(
                {"run_started_at": "2026-01-02T00:00:00Z"},
                datetime(2026, 1, 1, tzinfo=timezone.utc),
            )

    def test_terminal_check_reports_slow_attempts_without_failing(self):
        environment = {
            "GITHUB_EVENT_NAME": "pull_request",
            "GITHUB_API_URL": "https://api.github.com",
            "GITHUB_REPOSITORY": "idleai/test",
            "GITHUB_RUN_ID": "42",
            "GITHUB_RUN_ATTEMPT": "2",
            "GH_TOKEN": "test-token",
        }
        for seconds in [599, 601]:
            now = datetime(
                2026, 1, 2, 0, seconds // 60, seconds % 60, tzinfo=timezone.utc
            )
            with (
                self.subTest(seconds=seconds),
                patch.dict(os.environ, environment, clear=True),
                patch(
                    "budget.urlopen",
                    return_value=StringIO('{"run_started_at":"2026-01-02T00:00:00Z"}'),
                ) as request,
                patch("budget.datetime", wraps=datetime) as clock,
                patch("sys.stdout", new_callable=StringIO) as output,
            ):
                clock.now.return_value = now
                main()
                self.assertIn(f"PR feedback: {seconds}s", output.getvalue())
                self.assertIn("advisory target: 600s", output.getvalue())
                self.assertEqual("::notice::" in output.getvalue(), seconds > 600)
                self.assertTrue(
                    request.call_args.args[0].full_url.endswith("/runs/42/attempts/2")
                )
