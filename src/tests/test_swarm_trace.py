from __future__ import annotations

import unittest
from pathlib import Path
from uuid import uuid4

from openagent.integrations.swarm import OpenAgentRunner
from swarm import AgentResult, AgentSpec, RunContext, SwarmRuntime, build_function_registry, load_swarm_config

from _mock_model import ScriptedLanguageModel
from test_loop import _make_model_metadata


class SwarmTraceTests(unittest.IsolatedAsyncioTestCase):
    async def test_trace_records_run_task_runner_and_runner_events(self) -> None:
        config = load_swarm_config(
            {
                "fanout_budget": {"max_concurrent": 2},
                "runners": {
                    "alpha": {"kind": "function", "roles": ["research"]},
                    "beta": {"kind": "function", "roles": ["research"]},
                },
                "tasks": {
                    "trace-task": {
                        "role": "research",
                        "objective": "Trace fanout.",
                        "context": "Trace context.",
                        "boundaries": "Read-only.",
                        "output_schema": {"type": "object"},
                        "runner_ids": ["alpha", "beta"],
                    }
                },
            }
        )

        def ok(spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            return AgentResult(status="completed", summary=f"done:{spec.role}")

        registry = build_function_registry(config, {"alpha": ok, "beta": ok})
        result = await SwarmRuntime(registry=registry, fanout_budget=config.fanout_budget).run_task(
            config.task("trace-task"),
            run_id="trace-run",
        )

        events = result.trace_events
        by_name = _events_by_name(events)
        self.assertEqual(result.status, "completed")
        self.assertEqual(len(by_name["swarm.run.started"]), 1)
        self.assertEqual(len(by_name["swarm.task.started"]), 1)
        self.assertEqual(len(by_name["swarm.runner.started"]), 2)
        self.assertEqual(len(by_name["runner.started"]), 2)
        self.assertEqual(len(by_name["runner.finished"]), 2)
        self.assertEqual(len(by_name["swarm.runner.finished"]), 2)

        run_span = by_name["swarm.run.started"][0].span_id
        task_span = by_name["swarm.task.started"][0].span_id
        self.assertIsNone(by_name["swarm.run.started"][0].parent_span_id)
        self.assertEqual(by_name["swarm.task.started"][0].parent_span_id, run_span)
        self.assertTrue(all(event.parent_span_id == task_span for event in by_name["swarm.runner.started"]))

        runner_span_ids = {event.runner_id: event.span_id for event in by_name["swarm.runner.started"]}
        for event in by_name["runner.started"] + by_name["runner.finished"]:
            self.assertEqual(event.parent_span_id, runner_span_ids[event.runner_id])
            self.assertEqual(event.kind, "runner_event")
            self.assertEqual(event.run_id, "trace-run")

    async def test_trace_marks_failed_runner_and_aggregate_status(self) -> None:
        config = load_swarm_config(
            {
                "runners": {"broken": {"kind": "function", "roles": ["worker"]}},
                "tasks": {
                    "broken-task": {
                        "role": "worker",
                        "objective": "Break.",
                        "context": "Trace context.",
                        "boundaries": "Read-only.",
                        "output_schema": {"type": "object"},
                        "runner_ids": ["broken"],
                    }
                },
            }
        )

        def broken(_spec: AgentSpec, _ctx: RunContext) -> str:
            raise RuntimeError("boom")

        result = await SwarmRuntime(registry=build_function_registry(config, {"broken": broken})).run_task(config.task("broken-task"))
        by_name = _events_by_name(result.trace_events)

        self.assertEqual(result.status, "failed")
        self.assertEqual(by_name["swarm.runner.finished"][0].status, "error")
        self.assertEqual(by_name["swarm.task.finished"][0].status, "error")
        self.assertEqual(by_name["swarm.run.finished"][0].status, "error")
        self.assertEqual(by_name["runner.finished"][0].attributes["status"], "failed")

    async def test_trace_records_openagent_adapter_events_under_runner_span(self) -> None:
        workspace = _workspace()
        self.addCleanup(lambda: _cleanup(workspace))
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "openagent result"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 3, "output_tokens": 2}},
                ]
            ]
        )
        registry = load_swarm_openagent_registry(workspace, model)
        config = load_swarm_config(
            {
                "tasks": {
                    "oa-task": {
                        "role": "research",
                        "objective": "Run OpenAgent.",
                        "context": "Trace context.",
                        "boundaries": "Read-only.",
                        "output_schema": {"type": "object"},
                        "runner_ids": ["oa"],
                    }
                }
            }
        )

        result = await SwarmRuntime(registry=registry).run_task(config.task("oa-task"), run_id="oa-trace")
        by_name = _events_by_name(result.trace_events)
        runner_span = by_name["swarm.runner.started"][0].span_id

        self.assertEqual(result.status, "completed")
        self.assertIn("openagent.text-delta", by_name)
        self.assertTrue(any(event.parent_span_id == runner_span for event in by_name["openagent.text-delta"]))
        self.assertTrue(any(event.name == "runner.finished" for event in result.trace_events))


def _events_by_name(events):
    by_name = {}
    for event in events:
        by_name.setdefault(event.name, []).append(event)
    return by_name


def _workspace() -> Path:
    root = Path("openagent/tests/workdir")
    root.mkdir(parents=True, exist_ok=True)
    path = (root / f"swarm_trace_{uuid4().hex}").resolve()
    path.mkdir(parents=True, exist_ok=True)
    return path


def _cleanup(path: Path) -> None:
    import shutil

    shutil.rmtree(path, ignore_errors=True)


def load_swarm_openagent_registry(workspace: Path, model: ScriptedLanguageModel):
    from swarm.registry import RunnerRegistry

    registry = RunnerRegistry()
    registry.register(
        OpenAgentRunner(
            runner_id="oa",
            roles=["research"],
            model=model,
            model_metadata=_make_model_metadata(context_window=32768, max_output=128),
            workspace_root=workspace,
        )
    )
    return registry


if __name__ == "__main__":
    unittest.main()
