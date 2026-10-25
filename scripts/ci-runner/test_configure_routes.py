import unittest

from configure_routes import ROLE_LABELS, routing_plan


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

    def test_allocations_are_four_two_two(self):
        plan = routing_plan(self.runners())
        self.assertEqual(
            [worker["role"] for worker in plan],
            ["codex-pr-linux-x64"] * 4
            + ["idle-pr-linux-x64"] * 2
            + ["idle-background-linux-x64"] * 2,
        )

    def test_each_worker_has_one_role_and_keeps_legacy_routes(self):
        for worker in routing_plan(self.runners()):
            self.assertEqual(set(worker["labels"]) & ROLE_LABELS, {worker["role"]})
            self.assertIn("idle-linux-x64", worker["labels"])
            self.assertNotIn("Linux", worker["labels"])

    def test_missing_workers_fail_before_configuration(self):
        with self.assertRaises(ValueError):
            routing_plan(self.runners()[:-1])


if __name__ == "__main__":
    unittest.main()
