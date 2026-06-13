from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from swarm import (
    AgentResult,
    AgentSpec,
    FileSwarmStateStore,
    FileTeamHandoffStore,
    RunContext,
    SwarmCoordinatorOptions,
    SwarmRuntime,
    build_function_registry,
    load_swarm_config,
    run_swarm_coordinator,
)


class SwarmCoordinatorTests(unittest.IsolatedAsyncioTestCase):
    def _workspace(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = (root / f"swarm_coordinator_{uuid4().hex}").resolve()
        path.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    async def test_coordinator_runs_task_and_saves_team_handoff_receipt(self) -> None:
        temp = self._workspace()
        handoffs = FileTeamHandoffStore(temp / "handoffs")
        config = _config(["alpha", "beta"])

        def alpha(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            return AgentResult(status="completed", summary="alpha done")

        def beta(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            return AgentResult(status="failed", summary="beta failed")

        result = await run_swarm_coordinator(
            runtime=SwarmRuntime(registry=build_function_registry(config, {"alpha": alpha, "beta": beta})),
            task=config.task("task"),
            options=SwarmCoordinatorOptions(run_id="coordinator-run"),
            team_handoff_store=handoffs,
        )

        self.assertEqual(result.run_result.status, "partial")
        self.assertTrue(result.receipt.handoff_saved)
        self.assertTrue(Path(str(result.receipt.handoff_path)).exists())
        self.assertEqual(result.receipt.pending_runner_ids, ["beta"])
        self.assertEqual(result.receipt.reusable_runner_ids, ["alpha"])
        self.assertEqual(handoffs.load_handoff("coordinator-run").pending_runner_ids, ["beta"])
        self.assertEqual(result.receipt.as_dict()["run_id"], "coordinator-run")

    async def test_coordinator_resumes_from_previous_handoff_and_state(self) -> None:
        temp = self._workspace()
        state_store = FileSwarmStateStore(temp / "state")
        handoffs = FileTeamHandoffStore(temp / "handoffs")
        config = _config(["alpha", "beta"])
        calls = {"alpha": 0, "beta": 0}

        def alpha(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            calls["alpha"] += 1
            return AgentResult(status="completed", summary="alpha done")

        def beta_first(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            calls["beta"] += 1
            return AgentResult(status="failed", summary="beta failed")

        await run_swarm_coordinator(
            runtime=SwarmRuntime(
                registry=build_function_registry(config, {"alpha": alpha, "beta": beta_first}),
                state_store=state_store,
            ),
            task=config.task("task"),
            options=SwarmCoordinatorOptions(run_id="resume-team"),
            team_handoff_store=handoffs,
        )

        def alpha_should_not_run(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            raise AssertionError("alpha should be reused from state")

        def beta_second(_spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            calls["beta"] += 1
            return AgentResult(status="completed", summary="beta recovered")

        resumed = await run_swarm_coordinator(
            runtime=SwarmRuntime(
                registry=build_function_registry(config, {"alpha": alpha_should_not_run, "beta": beta_second}),
                state_store=state_store,
            ),
            task=config.task("task"),
            options=SwarmCoordinatorOptions(run_id="resume-team"),
            team_handoff_store=handoffs,
        )

        self.assertEqual(resumed.run_result.status, "completed")
        self.assertEqual(calls, {"alpha": 1, "beta": 2})
        self.assertFalse(resumed.handoff.has_pending)
        self.assertEqual(resumed.receipt.reusable_runner_ids, ["alpha", "beta"])

    async def test_coordinator_evaluates_merge_without_apply_by_default(self) -> None:
        temp = self._workspace()
        source = temp / "source"
        base_dir = temp / "workers"
        source.mkdir()
        (source / "file.txt").write_text("base", encoding="utf-8")
        config = _config(["alpha"], source=source, base_dir=base_dir, merge_policy={"auto_approve": False})

        def alpha(spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            workspace = Path(str(spec.inputs["worker_workspace"]))
            (workspace / "file.txt").write_text("changed", encoding="utf-8")
            return AgentResult(status="completed", summary="changed")

        result = await run_swarm_coordinator(
            runtime=SwarmRuntime(registry=build_function_registry(config, {"alpha": alpha})),
            task=config.task("task"),
            options=SwarmCoordinatorOptions(run_id="merge-review", merge_enabled=True),
        )

        self.assertEqual(result.receipt.merge_decision, "needs_review")
        self.assertEqual(result.receipt.merge_applied_count, 0)
        self.assertEqual(result.receipt.merge_change_count, 1)
        self.assertEqual((source / "file.txt").read_text(encoding="utf-8"), "base")

    async def test_coordinator_applies_approved_merge_when_enabled(self) -> None:
        temp = self._workspace()
        source = temp / "source"
        target = temp / "target"
        base_dir = temp / "workers"
        source.mkdir()
        target.mkdir()
        (source / "file.txt").write_text("base", encoding="utf-8")
        (target / "file.txt").write_text("base", encoding="utf-8")
        config = _config(["alpha"], source=source, base_dir=base_dir, merge_policy={"auto_approve": True})

        def alpha(spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            workspace = Path(str(spec.inputs["worker_workspace"]))
            (workspace / "file.txt").write_text("changed", encoding="utf-8")
            return AgentResult(status="completed", summary="changed")

        result = await run_swarm_coordinator(
            runtime=SwarmRuntime(registry=build_function_registry(config, {"alpha": alpha})),
            task=config.task("task"),
            options=SwarmCoordinatorOptions(
                run_id="merge-apply",
                merge_enabled=True,
                merge_target_root=str(target),
                apply_approved_merge=True,
            ),
        )

        self.assertEqual(result.receipt.merge_decision, "approved")
        self.assertEqual(result.receipt.merge_applied_count, 1)
        self.assertEqual((target / "file.txt").read_text(encoding="utf-8"), "changed")
        self.assertEqual((source / "file.txt").read_text(encoding="utf-8"), "base")


def _config(
    runners: list[str],
    *,
    source: Path | None = None,
    base_dir: Path | None = None,
    merge_policy: dict[str, object] | None = None,
):
    metadata: dict[str, object] = {}
    if source is not None and base_dir is not None:
        metadata["isolation"] = {
            "enabled": True,
            "mode": "copy",
            "source_root": str(source),
            "base_dir": str(base_dir),
        }
    if merge_policy is not None:
        metadata["merge"] = {"approval": merge_policy}
    return load_swarm_config(
        {
            "runners": {runner: {"kind": "function", "roles": ["worker"]} for runner in runners},
            "tasks": {
                "task": {
                    "role": "worker",
                    "objective": "Run a coordinated swarm task.",
                    "context": "Coordinator test context.",
                    "boundaries": "Read-only unless isolated workspace is provided.",
                    "output_schema": {"type": "object"},
                    "runner_ids": runners,
                    "metadata": metadata,
                }
            },
        }
    )


if __name__ == "__main__":
    unittest.main()
