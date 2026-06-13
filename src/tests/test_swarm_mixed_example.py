from __future__ import annotations

import unittest

from examples.swarm_mixed_openagent_a2a import run_example


class SwarmMixedExampleTests(unittest.IsolatedAsyncioTestCase):
    async def test_mixed_openagent_a2a_example_runs_offline(self) -> None:
        payload = await run_example()

        self.assertEqual(payload["status"], "completed")
        self.assertEqual(set(payload["results"]), {"openagent_researcher", "a2a_reviewer"})
        self.assertIn("OpenAgent worker", payload["results"]["openagent_researcher"]["summary"])
        self.assertIn("A2A reviewer", payload["results"]["a2a_reviewer"]["summary"])
        self.assertEqual(payload["a2a_request_count"], 1)
        self.assertEqual(payload["receipt"]["runner_count"], 2)
        self.assertEqual(payload["receipt"]["runner_status_counts"], {"completed": 2})
        self.assertEqual(
            [item["runner_id"] for item in payload["receipt"]["runner_summaries"]],
            ["a2a_reviewer", "openagent_researcher"],
        )


if __name__ == "__main__":
    unittest.main()
