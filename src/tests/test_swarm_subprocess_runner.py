from __future__ import annotations

import json
import shutil
import sys
import unittest
from pathlib import Path
from uuid import uuid4

from swarm import SwarmRuntime, build_subprocess_registry, load_swarm_config


class SwarmSubprocessRunnerTests(unittest.IsolatedAsyncioTestCase):
    def _workspace(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = (root / f"subprocess_{uuid4().hex}").resolve()
        path.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    async def test_subprocess_runner_executes_json_protocol(self) -> None:
        workspace = self._workspace()
        script = _write_script(
            workspace,
            """
import json
import sys

payload = json.loads(sys.stdin.read())
spec = payload["spec"]
print(json.dumps({
    "status": "completed",
    "summary": "subprocess:" + spec["objective"],
    "evidence": ["cli.py:1"],
    "confidence": 0.9,
    "usage": {"input_tokens": 2, "output_tokens": 3, "cost": 0.01},
    "metadata": {"seen_run_id": payload["context"]["run_id"]},
}))
""",
        )
        config = _config(workspace, script)

        result = await SwarmRuntime(registry=build_subprocess_registry(config), fanout_budget=config.fanout_budget).run_task(
            config.task("task"),
            run_id="subprocess-run",
        )

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["cli"].summary, "subprocess:Run external CLI.")
        self.assertEqual(result.results["cli"].evidence, ["cli.py:1"])
        self.assertEqual(result.results["cli"].metadata["seen_run_id"], "subprocess-run")
        self.assertEqual(result.usage.input_tokens, 2)
        self.assertEqual(result.usage.output_tokens, 3)
        self.assertIn("runner.started", {event.name for event in result.trace_events})
        self.assertIn("runner.finished", {event.name for event in result.trace_events})

    async def test_subprocess_runner_accepts_plain_stdout_as_summary(self) -> None:
        workspace = self._workspace()
        script = _write_script(workspace, "print('plain summary')\n")
        config = _config(workspace, script)

        result = await SwarmRuntime(registry=build_subprocess_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["cli"].summary, "plain summary")
        self.assertEqual(result.results["cli"].metadata["stdout_format"], "text")

    async def test_subprocess_runner_captures_nonzero_exit(self) -> None:
        workspace = self._workspace()
        script = _write_script(
            workspace,
            """
import sys
print("bad stderr", file=sys.stderr)
raise SystemExit(7)
""",
        )
        config = _config(workspace, script)

        result = await SwarmRuntime(registry=build_subprocess_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertEqual(result.results["cli"].status, "failed")
        self.assertIn("bad stderr", result.results["cli"].summary)
        self.assertEqual(result.results["cli"].metadata["returncode"], 7)

    async def test_subprocess_runner_times_out(self) -> None:
        workspace = self._workspace()
        script = _write_script(
            workspace,
            """
import time
time.sleep(3)
print("late")
""",
        )
        config = _config(workspace, script, timeout_seconds=0.05)

        result = await SwarmRuntime(registry=build_subprocess_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertEqual(result.results["cli"].metadata["error_kind"], "subprocess_timeout")


def _write_script(workspace: Path, body: str) -> Path:
    path = workspace / "agent.py"
    path.write_text(body.strip() + "\n", encoding="utf-8")
    return path


def _config(workspace: Path, script: Path, *, timeout_seconds: float | None = None):
    metadata = {
        "command": [sys.executable, str(script)],
        "cwd": str(workspace),
    }
    if timeout_seconds is not None:
        metadata["timeout_seconds"] = timeout_seconds
    return load_swarm_config(
        {
            "runners": {
                "cli": {
                    "kind": "subprocess",
                    "roles": ["research"],
                    "metadata": metadata,
                }
            },
            "tasks": {
                "task": {
                    "role": "research",
                    "objective": "Run external CLI.",
                    "context": "Subprocess test context.",
                    "boundaries": "Read-only.",
                    "output_schema": {"type": "object"},
                    "runner_ids": ["cli"],
                }
            },
        }
    )


if __name__ == "__main__":
    unittest.main()
