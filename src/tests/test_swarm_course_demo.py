from __future__ import annotations

import json
import unittest
from pathlib import Path
from unittest.mock import patch

from examples.swarm_course_demo import real_model_cli_args, run_offline_example, run_real_model_cli


class SwarmCourseDemoTests(unittest.IsolatedAsyncioTestCase):
    async def test_course_demo_runs_offline(self) -> None:
        payload = await run_offline_example()

        self.assertEqual(payload["status"], "completed")
        self.assertEqual(
            payload["runner_kinds"],
            {
                "openagent_teacher": "openagent",
                "subprocess_checker": "subprocess",
            },
        )
        self.assertEqual(set(payload["results"]), {"openagent_teacher", "subprocess_checker"})
        self.assertIn("Course teacher", payload["results"]["openagent_teacher"]["summary"])
        self.assertIn("Subprocess worker", payload["results"]["subprocess_checker"]["summary"])
        self.assertEqual(payload["receipt"]["runner_count"], 2)
        self.assertEqual(payload["receipt"]["runner_status_counts"], {"completed": 2})
        self.assertEqual(
            [item["runner_id"] for item in payload["receipt"]["runner_summaries"]],
            ["openagent_teacher", "subprocess_checker"],
        )
        self.assertEqual(payload["demo"]["mode"], "offline")
        self.assertTrue(payload["demo"]["real_model_command"].startswith("openagent-swarm run "))
        self.assertIn("--enable-openagent", payload["demo"]["real_model_command"])
        self.assertGreater(payload["trace_event_count"], 0)
        flattened = json.dumps(payload, ensure_ascii=False)
        self.assertNotIn("openagent_trace", flattened)
        self.assertNotIn("workdir_swarm_course_demo/.openagent", flattened)

    async def test_real_model_command_is_openagent_swarm_binding(self) -> None:
        args = real_model_cli_args(config_path=Path("demo.yaml"), workspace=Path("."), run_id="course-real", pretty=False)

        self.assertEqual(args[:3], ["run", "demo.yaml", "--task"])
        self.assertIn("--enable-openagent", args)
        self.assertIn("--workspace", args)
        self.assertIn("course-real", args)
        self.assertNotIn("--pretty", args)

    async def test_real_model_mode_delegates_to_swarm_cli(self) -> None:
        captured: dict[str, list[str]] = {}

        def fake_main(argv: list[str]) -> int:
            captured["argv"] = argv
            return 0

        with patch("examples.swarm_course_demo.swarm_cli.main", fake_main):
            code = run_real_model_cli(["--workspace", "/tmp/course", "--run-id", "real-run", "--pretty"])

        self.assertEqual(code, 0)
        self.assertIn("--enable-openagent", captured["argv"])
        self.assertIn("/tmp/course", captured["argv"])
        self.assertIn("real-run", captured["argv"])
        self.assertIn("--pretty", captured["argv"])


if __name__ == "__main__":
    unittest.main()
