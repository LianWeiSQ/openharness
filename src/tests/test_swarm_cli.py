from __future__ import annotations

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from swarm import AgentDescriptor, AgentResult
from swarm import cli as swarm_cli
from swarm.function_runner import FunctionRunner
from swarm.registry import RunnerRegistry


class SwarmCliTests(unittest.TestCase):
    def test_cli_runs_yaml_subprocess_and_outputs_json(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            config = _write_config(tmp, worker=_write_worker(tmp, summary="worker completed"))

            code, stdout, stderr = _run_cli(["run", str(config), "--task", "demo", "--run-id", "cli-run"])

        self.assertEqual(code, 0, stderr)
        payload = json.loads(stdout)
        self.assertEqual(payload["run_id"], "cli-run")
        self.assertEqual(payload["task_id"], "demo")
        self.assertEqual(payload["status"], "completed")
        self.assertEqual(payload["results"]["worker"]["summary"], "worker completed")
        self.assertGreater(payload["trace_event_count"], 0)
        self.assertNotIn("trace_events", payload)

    def test_cli_writes_state_and_handoff_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            config = _write_config(tmp, worker=_write_worker(tmp, summary="persisted"))
            state_dir = tmp / "state"
            handoff_dir = tmp / "handoff"

            code, stdout, stderr = _run_cli(
                [
                    "run",
                    str(config),
                    "--task",
                    "demo",
                    "--run-id",
                    "persist-run",
                    "--state-dir",
                    str(state_dir),
                    "--handoff-dir",
                    str(handoff_dir),
                ]
            )

            state_path = state_dir / "persist-run" / "state.latest.json"
            trace_path = state_dir / "persist-run" / "trace.jsonl"
            handoff_path = handoff_dir / "persist-run" / "team-handoff.json"
            receipt_path = handoff_dir / "persist-run" / "coordinator-receipt.json"

            self.assertEqual(code, 0, stderr)
            payload = json.loads(stdout)
            self.assertEqual(payload["state_dir"], str(state_dir.resolve()))
            self.assertEqual(payload["receipt"]["run_id"], "persist-run")
            self.assertEqual(payload["receipt"]["runner_count"], 1)
            self.assertEqual(payload["receipt"]["runner_status_counts"], {"completed": 1})
            self.assertGreater(payload["receipt"]["trace_event_count"], 0)
            self.assertEqual(payload["receipt"]["runner_summaries"][0]["runner_id"], "worker")
            self.assertEqual(payload["receipt"]["runner_summaries"][0]["summary_preview"], "persisted")
            self.assertEqual(payload["receipt"]["receipt_path"], str(receipt_path.resolve()))
            self.assertEqual(payload["receipt"]["handoff_path"], str(handoff_path.resolve()))
            self.assertTrue(state_path.exists())
            self.assertTrue(trace_path.exists())
            self.assertTrue(handoff_path.exists())
            self.assertTrue(receipt_path.exists())

    def test_cli_returns_nonzero_for_failed_runner(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            config = _write_config(tmp, worker=_write_worker(tmp, summary="failed", status="failed"))

            code, stdout, stderr = _run_cli(["run", str(config), "--task", "demo"])

        self.assertEqual(code, 1, stderr)
        payload = json.loads(stdout)
        self.assertEqual(payload["status"], "failed")
        self.assertEqual(payload["results"]["worker"]["status"], "failed")

    def test_cli_ignores_unselected_runner_config(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            config = _write_config(tmp, worker=_write_worker(tmp, summary="selected only"), extra_runner=True)

            code, stdout, stderr = _run_cli(["run", str(config), "--task", "demo"])

        self.assertEqual(code, 0, stderr)
        payload = json.loads(stdout)
        self.assertEqual(payload["results"]["worker"]["summary"], "selected only")
        self.assertEqual(set(payload["results"]), {"worker"})

    def test_cli_rejects_function_runners_without_handler_binding(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            config = Path(raw_tmp) / "swarm.yaml"
            config.write_text(
                """
runners:
  worker:
    kind: function
    roles: [worker]
    handler: worker_fn
tasks:
  demo:
    role: worker
    objective: Run demo.
    context: Test.
    boundaries: Read-only.
    output_schema:
      type: object
    runner_ids: [worker]
""",
                encoding="utf-8",
            )

            code, stdout, stderr = _run_cli(["run", str(config), "--task", "demo"])

        self.assertEqual(code, 2)
        self.assertEqual(stdout, "")
        error = json.loads(stderr)
        self.assertEqual(error["status"], "error")
        self.assertIn("not supported by the config-only CLI", error["error"])

    def test_cli_rejects_openagent_runner_without_enable_flag(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            config = Path(raw_tmp) / "swarm.yaml"
            config.write_text(
                """
runners:
  oa:
    kind: openagent
    roles: [worker]
tasks:
  demo:
    role: worker
    objective: Run demo.
    context: Test.
    boundaries: Read-only.
    output_schema:
      type: object
    runner_ids: [oa]
""",
                encoding="utf-8",
            )

            code, stdout, stderr = _run_cli(["run", str(config), "--task", "demo"])

        self.assertEqual(code, 2)
        self.assertEqual(stdout, "")
        error = json.loads(stderr)
        self.assertIn("--enable-openagent", error["error"])

    def test_cli_can_enable_openagent_runner_binding(self) -> None:
        captured: dict[str, object] = {}

        async def fake_openagent_registry(*, config, args):
            captured["runner_ids"] = [runner.id for runner in config.runners]
            captured["workspace"] = args.workspace
            captured["model"] = args.model
            registry = RunnerRegistry()
            registry.register(
                FunctionRunner(
                    descriptor=AgentDescriptor(id="oa", roles=["worker"], kind="openagent", supports_streaming=True),
                    handler=lambda _spec, _ctx: AgentResult(status="completed", summary="openagent cli bound"),
                )
            )
            return registry

        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            config = tmp / "swarm.yaml"
            config.write_text(
                """
runners:
  oa:
    kind: openagent
    roles: [worker]
  unused_http:
    kind: http
    roles: [other]
    metadata: {}
tasks:
  demo:
    role: worker
    objective: Run demo.
    context: Test.
    boundaries: Read-only.
    output_schema:
      type: object
    runner_ids: [oa]
""",
                encoding="utf-8",
            )

            with patch.object(swarm_cli, "_build_openagent_registry_from_cli", fake_openagent_registry):
                code, stdout, stderr = _run_cli(
                    [
                        "run",
                        str(config),
                        "--task",
                        "demo",
                        "--enable-openagent",
                        "--workspace",
                        str(tmp),
                        "--model",
                        "gpt-test",
                    ]
                )

        self.assertEqual(code, 0, stderr)
        payload = json.loads(stdout)
        self.assertEqual(payload["results"]["oa"]["summary"], "openagent cli bound")
        self.assertEqual(captured["runner_ids"], ["oa"])
        self.assertEqual(captured["workspace"], str(tmp))
        self.assertEqual(captured["model"], "gpt-test")


def _run_cli(argv: list[str]) -> tuple[int, str, str]:
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        code = swarm_cli.main(argv)
    return code, stdout.getvalue(), stderr.getvalue()


def _write_worker(tmp: Path, *, summary: str, status: str = "completed") -> Path:
    worker = tmp / f"worker_{status}.py"
    worker.write_text(
        f"""
from __future__ import annotations

import json
import sys

payload = json.loads(sys.stdin.read())
assert payload["spec"]["objective"] == "Run demo."
print(json.dumps({{"status": {status!r}, "summary": {summary!r}, "evidence": ["worker.py"]}}))
""",
        encoding="utf-8",
    )
    return worker


def _write_config(tmp: Path, *, worker: Path, extra_runner: bool = False) -> Path:
    config = tmp / "swarm.yaml"
    extra = """
  unused_http:
    kind: http
    roles: [other]
    metadata: {}
""" if extra_runner else ""
    config.write_text(
        f"""
fanout_budget:
  max_concurrent: 1
runners:
  worker:
    kind: subprocess
    roles: [worker]
    metadata:
      command:
        - {json.dumps(sys.executable)}
        - {json.dumps(str(worker))}
{extra.rstrip()}
tasks:
  demo:
    role: worker
    objective: Run demo.
    context: Test.
    boundaries: Read-only.
    output_schema:
      type: object
    runner_ids: [worker]
""",
        encoding="utf-8",
    )
    return config


if __name__ == "__main__":
    unittest.main()
