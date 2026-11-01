import unittest

from configure_routes import (
    REPOSITORIES,
    ROLE_LABELS,
    SHARED_ALIASES,
    SHARED_LABEL,
    repository_routes,
    routing_plan,
)


class RoutingTests(unittest.TestCase):
    def runners(self):
        return [
            {
                "id": index,
                "name": f"idle-linux-local-{index:02}",
                "labels": [
                    {"name": "Linux", "type": "read-only"},
                    {"name": "idle-linux-x64", "type": "custom"},
                    {"name": "idle-pr-linux-x64", "type": "custom"},
                ],
            }
            for index in range(1, 9)
        ]

    def test_allocations_are_four_codex_and_four_shared(self):
        plan = routing_plan(self.runners())
        self.assertEqual(
            [worker["role"] for worker in plan],
            ["codex-pr-linux-x64"] * 4 + [SHARED_LABEL] * 4,
        )

    def test_queued_and_current_shared_routes_use_the_same_four_workers(self):
        plan = routing_plan(self.runners())
        for label in {SHARED_LABEL, *SHARED_ALIASES}:
            self.assertEqual(
                [worker["name"] for worker in plan if label in worker["labels"]],
                [f"idle-linux-local-{index:02}" for index in range(5, 9)],
            )

    def test_roles_remain_isolated_and_keep_unrelated_custom_labels(self):
        for worker in routing_plan(self.runners()):
            expected = {worker["role"]}
            if worker["role"] == SHARED_LABEL:
                expected.update(SHARED_ALIASES)
            self.assertEqual(set(worker["labels"]) & ROLE_LABELS, expected)
            self.assertIn("idle-linux-x64", worker["labels"])
            self.assertNotIn("Linux", worker["labels"])

    def test_reapplying_routes_preserves_the_assignments(self):
        plan = routing_plan(self.runners())
        applied = [
            dict(
                worker,
                labels=[
                    {"name": label, "type": "custom"} for label in worker["labels"]
                ],
            )
            for worker in plan
        ]
        self.assertEqual(routing_plan(applied), plan)

    def test_repository_routes_share_capacity_except_codex_prs(self):
        self.assertIn("memos", REPOSITORIES)
        for repository in REPOSITORIES:
            routes = dict(repository_routes(repository))
            self.assertEqual(routes["IDLE_BACKGROUND_RUNNER"], SHARED_LABEL)
            if repository == "codex-evo":
                self.assertEqual(routes["CODEX_LINUX_RUNNER"], "codex-pr-linux-x64")
                self.assertNotIn("IDLE_LINUX_RUNNER", routes)
            else:
                self.assertEqual(routes["IDLE_LINUX_RUNNER"], SHARED_LABEL)

    def test_missing_workers_fail_before_configuration(self):
        with self.assertRaises(ValueError):
            routing_plan(self.runners()[:-1])


if __name__ == "__main__":
    unittest.main()
