from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from swarm import (
    AgentResult,
    AgentSpec,
    FileSwarmStateStore,
    RunContext,
    SwarmRuntime,
    Usage,
    build_function_registry,
    load_swarm_config,
)


class SwarmResumeTests(unittest.IsolatedAsyncioTestCase):
    def _workspace(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = (root / f"swarm_resume_{uuid4().hex}").resolve()
        path.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    async def test_resume_reuses_completed_result_and_reruns_failed_result(self) -> None:
        store = FileSwarmStateStore(self._workspace() / "state")
        config = _config(resume=True)
        calls = {"alpha": 0, "beta": 0}

        def alpha(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            calls["alpha"] += 1
            return AgentResult(
                status="completed",
                summary="alpha complete",
                evidence=["alpha.py:1"],
                usage=Usage(input_tokens=2, output_tokens=3),
            )

        def beta_first(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            calls["beta"] += 1
            return AgentResult(status="failed", summary="beta failed once")

        first = await SwarmRuntime(
            registry=build_function_registry(config, {"alpha": alpha, "beta": beta_first}),
            state_store=store,
        ).run_task(config.task("task"), run_id="resume-run")

        self.assertEqual(first.status, "partial")
        self.assertEqual(calls, {"alpha": 1, "beta": 1})

        def alpha_should_not_run(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            raise AssertionError("alpha should have been resumed")

        def beta_second(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            calls["beta"] += 1
            return AgentResult(status="completed", summary="beta recovered", usage=Usage(output_tokens=1))

        second = await SwarmRuntime(
            registry=build_function_registry(config, {"alpha": alpha_should_not_run, "beta": beta_second}),
            state_store=store,
        ).run_task(config.task("task"), run_id="resume-run")

        self.assertEqual(second.status, "completed")
        self.assertEqual(calls, {"alpha": 1, "beta": 2})
        self.assertEqual(second.results["alpha"].summary, "alpha complete")
        self.assertTrue(second.results["alpha"].metadata["resumed"])
        self.assertEqual(second.results["alpha"].metadata["resume_reused_status"], "completed")
        self.assertEqual(second.results["beta"].summary, "beta recovered")
        self.assertIn("resumed 1 runner result(s) from previous state", second.warnings)
        resume_events = [event for event in second.trace_events if event.name == "swarm.resume"]
        self.assertEqual(len(resume_events), 1)
        self.assertEqual(resume_events[0].attributes["reused_runner_ids"], ["alpha"])
        self.assertEqual(resume_events[0].attributes["dispatch_runner_ids"], ["beta"])

        loaded = store.load_run("resume-run")
        self.assertEqual(loaded["results"]["alpha"]["metadata"]["resumed"], True)
        self.assertEqual(loaded["results"]["beta"]["summary"], "beta recovered")

    async def test_resume_can_skip_all_runners(self) -> None:
        store = FileSwarmStateStore(self._workspace() / "state")
        config = _config(resume=False)
        calls = {"alpha": 0}

        def alpha(_spec: AgentSpec, _ctx: RunContext) -> str:
            calls["alpha"] += 1
            return "done once"

        await SwarmRuntime(
            registry=build_function_registry(config, {"alpha": alpha, "beta": _unused_beta}),
            state_store=store,
        ).run_task(config.task("single"), run_id="all-reused")

        def alpha_should_not_run(_spec: AgentSpec, _ctx: RunContext) -> str:
            raise AssertionError("alpha should have been resumed by runtime policy")

        second = await SwarmRuntime(
            registry=build_function_registry(config, {"alpha": alpha_should_not_run, "beta": _unused_beta}),
            state_store=store,
            resume_policy=True,
        ).run_task(config.task("single"), run_id="all-reused")

        self.assertEqual(second.status, "completed")
        self.assertEqual(second.results["alpha"].summary, "done once")
        self.assertEqual(calls["alpha"], 1)
        self.assertIn("resumed 1 runner result(s) from previous state", second.warnings)

    async def test_resume_is_disabled_by_default(self) -> None:
        store = FileSwarmStateStore(self._workspace() / "state")
        config = _config(resume=False)
        calls = {"alpha": 0}

        def alpha(_spec: AgentSpec, _ctx: RunContext) -> str:
            calls["alpha"] += 1
            return f"call {calls['alpha']}"

        runtime = SwarmRuntime(
            registry=build_function_registry(config, {"alpha": alpha, "beta": _unused_beta}),
            state_store=store,
        )

        first = await runtime.run_task(config.task("single"), run_id="fresh-run")
        second = await runtime.run_task(config.task("single"), run_id="fresh-run")

        self.assertEqual(first.results["alpha"].summary, "call 1")
        self.assertEqual(second.results["alpha"].summary, "call 2")
        self.assertEqual(calls["alpha"], 2)
        self.assertFalse(second.warnings)

    async def test_resume_ignores_state_for_different_task_id(self) -> None:
        store = FileSwarmStateStore(self._workspace() / "state")
        config = _config(resume=True)
        calls = {"alpha": 0}

        def alpha(_spec: AgentSpec, _ctx: RunContext) -> str:
            calls["alpha"] += 1
            return f"fresh {calls['alpha']}"

        runtime = SwarmRuntime(
            registry=build_function_registry(config, {"alpha": alpha, "beta": _unused_beta}),
            state_store=store,
        )

        first = await runtime.run_task(config.task("single"), run_id="task-mismatch")
        second = await runtime.run_task(config.task("other"), run_id="task-mismatch")

        self.assertEqual(first.results["alpha"].summary, "fresh 1")
        self.assertEqual(second.results["alpha"].summary, "fresh 2")
        self.assertEqual(calls["alpha"], 2)
        self.assertIn('resume state task_id "single" did not match "other"; running fresh', second.warnings)


def _config(*, resume: bool):
    metadata = {"resume": {"enabled": True, "reuse_statuses": ["completed"]}} if resume else {}
    return load_swarm_config(
        {
            "runners": {
                "alpha": {"kind": "function", "roles": ["worker"]},
                "beta": {"kind": "function", "roles": ["worker"]},
            },
            "tasks": {
                "task": {
                    "role": "worker",
                    "objective": "Resume a two-runner task.",
                    "context": "A previous run may have completed one worker.",
                    "boundaries": "Read-only.",
                    "output_schema": {"type": "object"},
                    "runner_ids": ["alpha", "beta"],
                    "metadata": metadata,
                },
                "single": {
                    "role": "worker",
                    "objective": "Resume a single-runner task.",
                    "context": "A previous run may already be complete.",
                    "boundaries": "Read-only.",
                    "output_schema": {"type": "object"},
                    "runner_ids": ["alpha"],
                    "metadata": metadata,
                },
                "other": {
                    "role": "worker",
                    "objective": "Do not resume a different task.",
                    "context": "The task id differs from the saved state.",
                    "boundaries": "Read-only.",
                    "output_schema": {"type": "object"},
                    "runner_ids": ["alpha"],
                    "metadata": metadata,
                },
            },
        }
    )


def _unused_beta(_spec: AgentSpec, _ctx: RunContext) -> str:
    raise AssertionError("beta should not be dispatched for this task")


if __name__ == "__main__":
    unittest.main()
