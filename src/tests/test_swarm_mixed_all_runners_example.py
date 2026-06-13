from __future__ import annotations

import json
import unittest

from examples.swarm_mixed_all_runners import run_example


class SwarmMixedAllRunnersExampleTests(unittest.IsolatedAsyncioTestCase):
    async def test_mixed_all_runners_example_runs_offline(self) -> None:
        payload = await run_example()

        self.assertEqual(payload["status"], "completed")
        self.assertEqual(
            payload["runner_kinds"],
            {
                "openagent_researcher": "openagent",
                "subprocess_checker": "subprocess",
                "http_planner": "http",
                "a2a_reviewer": "a2a",
            },
        )
        self.assertEqual(
            set(payload["results"]),
            {"openagent_researcher", "subprocess_checker", "http_planner", "a2a_reviewer"},
        )
        self.assertIn("OpenAgent worker", payload["results"]["openagent_researcher"]["summary"])
        self.assertIn("Subprocess worker", payload["results"]["subprocess_checker"]["summary"])
        self.assertIn("HTTP worker", payload["results"]["http_planner"]["summary"])
        self.assertIn("A2A reviewer", payload["results"]["a2a_reviewer"]["summary"])
        self.assertEqual(payload["mock_request_counts"], {"http": 1, "a2a": 1})
        self.assertEqual(payload["results"]["subprocess_checker"]["metadata"]["stdout_format"], "json")
        self.assertEqual(payload["results"]["http_planner"]["metadata"]["http_status"], 200)
        self.assertEqual(payload["results"]["a2a_reviewer"]["metadata"]["a2a_task_id"], "a2a-mixed-demo-task")
        flattened = json.dumps(payload, ensure_ascii=False)
        self.assertNotIn("openagent_trace", flattened)
        self.assertNotIn("workdir_swarm_mixed_all/.openagent", flattened)
        self.assertEqual(payload["receipt"]["runner_count"], 4)
        self.assertEqual(payload["receipt"]["runner_status_counts"], {"completed": 4})
        self.assertEqual(
            [item["runner_id"] for item in payload["receipt"]["runner_summaries"]],
            ["a2a_reviewer", "http_planner", "openagent_researcher", "subprocess_checker"],
        )
        self.assertGreater(payload["trace_event_count"], 0)


if __name__ == "__main__":
    unittest.main()
