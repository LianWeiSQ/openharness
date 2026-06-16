from __future__ import annotations

import json
import tempfile
import threading
import unittest
import urllib.request
from pathlib import Path
from unittest.mock import patch

from examples.swarm_course_demo import display_inspect_command, real_model_cli_args, run_offline_example, run_real_model_cli
from swarm.inspection import SwarmInspectionConfig, create_inspection_server, load_run_detail, load_run_index


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
        self.assertFalse(payload["demo"]["persistence"]["enabled"])
        self.assertIsNone(payload["demo"]["inspect_command"])
        self.assertGreater(payload["trace_event_count"], 0)
        flattened = json.dumps(payload, ensure_ascii=False)
        self.assertNotIn("openagent_trace", flattened)
        self.assertNotIn("workdir_swarm_course_demo/.openagent", flattened)

    async def test_course_demo_can_persist_for_inspection(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            state_dir = tmp / "state"
            handoff_dir = tmp / "handoff"
            payload = await run_offline_example(
                run_id="course-persist",
                persist=True,
                state_dir=state_dir,
                handoff_dir=handoff_dir,
                inspect_port=8877,
            )

            state_path = state_dir / "course-persist" / "state.latest.json"
            handoff_path = handoff_dir / "course-persist" / "team-handoff.json"
            receipt_path = handoff_dir / "course-persist" / "coordinator-receipt.json"
            index = load_run_index(SwarmInspectionConfig(state_dir=state_dir, handoff_dir=handoff_dir))
            detail = load_run_detail(SwarmInspectionConfig(state_dir=state_dir, handoff_dir=handoff_dir), "course-persist")

            self.assertEqual(payload["status"], "completed")
            self.assertTrue(state_path.exists())
            self.assertTrue(handoff_path.exists())
            self.assertTrue(receipt_path.exists())
            self.assertEqual(payload["demo"]["persistence"]["state_path"], str(state_path.resolve()))
            self.assertEqual(payload["demo"]["persistence"]["handoff_path"], str(handoff_path.resolve()))
            self.assertEqual(payload["demo"]["persistence"]["receipt_path"], str(receipt_path.resolve()))
            self.assertIn("--state-dir", payload["demo"]["inspect_command"])
            self.assertIn("--handoff-dir", payload["demo"]["inspect_command"])
            self.assertIn("8877", payload["demo"]["inspect_command"])
            self.assertEqual(index["run_count"], 1)
            self.assertEqual(index["runs"][0]["run_id"], "course-persist")
            self.assertIsNotNone(detail)
            self.assertEqual(detail["receipt"]["run_id"], "course-persist")
            self.assertEqual(detail["handoff"]["run_id"], "course-persist")
            self.assertEqual(detail["state"]["run_id"], "course-persist")

    async def test_course_demo_persisted_run_serves_inspection_http_routes(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            state_dir = tmp / "state"
            handoff_dir = tmp / "handoff"
            await run_offline_example(run_id="course-http", persist=True, state_dir=state_dir, handoff_dir=handoff_dir)
            server = create_inspection_server(SwarmInspectionConfig(state_dir=state_dir, handoff_dir=handoff_dir), port=0)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            host, port = server.server_address
            base = f"http://{host}:{port}"

            try:
                health = _get_json(f"{base}/health")
                runs = _get_json(f"{base}/runs")
                detail = _get_json(f"{base}/runs/course-http")
                receipt = _get_json(f"{base}/runs/course-http/receipt")
                trace = _get_json(f"{base}/runs/course-http/trace")
                html = _get_text(f"{base}/")
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=1)

        self.assertEqual(health["status"], "ok")
        self.assertEqual(runs["run_count"], 1)
        self.assertEqual(runs["runs"][0]["run_id"], "course-http")
        self.assertEqual(detail["run"]["run_id"], "course-http")
        self.assertEqual(detail["receipt"]["run_id"], "course-http")
        self.assertEqual(detail["handoff"]["run_id"], "course-http")
        self.assertEqual(receipt["runner_status_counts"], {"completed": 2})
        self.assertGreater(len(trace), 0)
        self.assertIn("Swarm Inspection", html)
        self.assertIn("Runner Status", html)

    async def test_real_model_command_is_openagent_swarm_binding(self) -> None:
        args = real_model_cli_args(
            config_path=Path("demo.yaml"),
            workspace=Path("."),
            run_id="course-real",
            state_dir=Path("state"),
            handoff_dir=Path("handoff"),
            pretty=False,
        )

        self.assertEqual(args[:3], ["run", "demo.yaml", "--task"])
        self.assertIn("--enable-openagent", args)
        self.assertIn("--workspace", args)
        self.assertIn("--state-dir", args)
        self.assertIn("--handoff-dir", args)
        self.assertIn("course-real", args)
        self.assertNotIn("--pretty", args)

    async def test_display_inspect_command_is_copyable(self) -> None:
        command = display_inspect_command(state_dir=Path("state dir"), handoff_dir=Path("handoff dir"), port=8899)

        self.assertTrue(command.startswith("openagent-swarm inspect "))
        self.assertIn("--state-dir 'state dir'", command)
        self.assertIn("--handoff-dir 'handoff dir'", command)
        self.assertIn("--port 8899", command)

    async def test_real_model_mode_delegates_to_swarm_cli(self) -> None:
        captured: dict[str, list[str]] = {}

        def fake_main(argv: list[str]) -> int:
            captured["argv"] = argv
            return 0

        with patch("examples.swarm_course_demo.swarm_cli.main", fake_main):
            code = run_real_model_cli(
                [
                    "--workspace",
                    "/tmp/course",
                    "--run-id",
                    "real-run",
                    "--state-dir",
                    "/tmp/state",
                    "--handoff-dir",
                    "/tmp/handoff",
                    "--pretty",
                ]
            )

        self.assertEqual(code, 0)
        self.assertIn("--enable-openagent", captured["argv"])
        self.assertIn("/tmp/course", captured["argv"])
        self.assertIn("real-run", captured["argv"])
        self.assertIn("/tmp/state", captured["argv"])
        self.assertIn("/tmp/handoff", captured["argv"])
        self.assertIn("--pretty", captured["argv"])


def _get_json(url: str):
    with urllib.request.urlopen(url, timeout=5) as response:  # noqa: S310 - local test server.
        assert "application/json" in response.headers["content-type"]
        return json.loads(response.read().decode("utf-8"))


def _get_text(url: str) -> str:
    with urllib.request.urlopen(url, timeout=5) as response:  # noqa: S310 - local test server.
        assert "text/html" in response.headers["content-type"]
        return response.read().decode("utf-8")


if __name__ == "__main__":
    unittest.main()
