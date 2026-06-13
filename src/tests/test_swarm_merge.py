from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from swarm import (
    AgentResult,
    AgentSpec,
    RunContext,
    SwarmRuntime,
    apply_merge_plan,
    build_function_registry,
    build_merge_plan,
    load_swarm_config,
)


class SwarmMergeTests(unittest.IsolatedAsyncioTestCase):
    def _workspace(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = (root / f"swarm_merge_{uuid4().hex}").resolve()
        path.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    async def test_runtime_preserves_worker_workspace_metadata_on_results(self) -> None:
        temp = self._workspace()
        source = temp / "source"
        source.mkdir()
        (source / "file.txt").write_text("source", encoding="utf-8")
        base_dir = temp / "isolated"

        def worker(spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            workspace = Path(str(spec.inputs["worker_workspace"]))
            (workspace / "file.txt").write_text("changed", encoding="utf-8")
            return AgentResult(status="completed", summary="changed")

        config = _config(source=source, base_dir=base_dir, runners=["alpha"])
        result = await SwarmRuntime(registry=build_function_registry(config, {"alpha": worker})).run_task(config.task("task"))

        metadata = result.results["alpha"].metadata
        self.assertTrue(metadata["workspace_isolated"])
        self.assertEqual(metadata["workspace_source_root"], str(source))
        self.assertTrue(Path(str(metadata["worker_workspace"])).exists())

    def test_build_merge_plan_detects_added_modified_and_deleted_files(self) -> None:
        temp = self._workspace()
        source = temp / "source"
        worker = temp / "worker"
        source.mkdir()
        worker.mkdir()
        (source / "modify.txt").write_text("base", encoding="utf-8")
        (source / "delete.txt").write_text("remove me", encoding="utf-8")
        (source / "same.txt").write_text("same", encoding="utf-8")
        (worker / "modify.txt").write_text("changed", encoding="utf-8")
        (worker / "same.txt").write_text("same", encoding="utf-8")
        (worker / "add.txt").write_text("new", encoding="utf-8")
        results = {"alpha": _result_for_worker(worker, source)}

        plan = build_merge_plan(results)

        changes = {(change.relative_path, change.change_type) for change in plan.changes}
        self.assertEqual(changes, {("add.txt", "added"), ("delete.txt", "deleted"), ("modify.txt", "modified")})
        self.assertFalse(plan.has_conflicts)

    def test_merge_plan_detects_same_path_conflicts_and_apply_skips_them(self) -> None:
        temp = self._workspace()
        source = temp / "source"
        alpha = temp / "alpha"
        beta = temp / "beta"
        target = temp / "target"
        for path in (source, alpha, beta, target):
            path.mkdir()
        (source / "conflict.txt").write_text("base", encoding="utf-8")
        (source / "safe.txt").write_text("base", encoding="utf-8")
        shutil.copytree(source, alpha, dirs_exist_ok=True)
        shutil.copytree(source, beta, dirs_exist_ok=True)
        shutil.copytree(source, target, dirs_exist_ok=True)
        (alpha / "conflict.txt").write_text("alpha", encoding="utf-8")
        (beta / "conflict.txt").write_text("beta", encoding="utf-8")
        (alpha / "safe.txt").write_text("safe change", encoding="utf-8")
        results = {
            "alpha": _result_for_worker(alpha, source),
            "beta": _result_for_worker(beta, source),
        }

        plan = build_merge_plan(results)
        applied = apply_merge_plan(plan, target_root=target)

        self.assertTrue(plan.has_conflicts)
        self.assertEqual(plan.conflicts[0].relative_path, "conflict.txt")
        self.assertEqual(set(plan.conflicts[0].runner_ids), {"alpha", "beta"})
        self.assertEqual(applied.skipped_conflicts, ["conflict.txt"])
        self.assertEqual((target / "conflict.txt").read_text(encoding="utf-8"), "base")
        self.assertEqual((target / "safe.txt").read_text(encoding="utf-8"), "safe change")

    def test_apply_merge_plan_applies_non_conflicting_added_modified_and_deleted_files(self) -> None:
        temp = self._workspace()
        source = temp / "source"
        worker = temp / "worker"
        target = temp / "target"
        for path in (source, worker, target):
            path.mkdir()
        (source / "modify.txt").write_text("base", encoding="utf-8")
        (source / "delete.txt").write_text("remove me", encoding="utf-8")
        shutil.copytree(source, worker, dirs_exist_ok=True)
        shutil.copytree(source, target, dirs_exist_ok=True)
        (worker / "modify.txt").write_text("changed", encoding="utf-8")
        (worker / "delete.txt").unlink()
        (worker / "add.txt").write_text("new", encoding="utf-8")

        plan = build_merge_plan({"alpha": _result_for_worker(worker, source)})
        applied = apply_merge_plan(plan, target_root=target)

        self.assertEqual({change.relative_path for change in applied.applied}, {"add.txt", "delete.txt", "modify.txt"})
        self.assertEqual((target / "modify.txt").read_text(encoding="utf-8"), "changed")
        self.assertEqual((target / "add.txt").read_text(encoding="utf-8"), "new")
        self.assertFalse((target / "delete.txt").exists())

    def test_build_merge_plan_rejects_mixed_source_roots_without_explicit_source(self) -> None:
        temp = self._workspace()
        source_a = temp / "source_a"
        source_b = temp / "source_b"
        worker_a = temp / "worker_a"
        worker_b = temp / "worker_b"
        for path in (source_a, source_b, worker_a, worker_b):
            path.mkdir()

        with self.assertRaises(ValueError):
            build_merge_plan(
                {
                    "alpha": _result_for_worker(worker_a, source_a),
                    "beta": _result_for_worker(worker_b, source_b),
                }
            )


def _config(*, source: Path, base_dir: Path, runners: list[str]):
    return load_swarm_config(
        {
            "runners": {runner: {"kind": "function", "roles": ["worker"]} for runner in runners},
            "tasks": {
                "task": {
                    "role": "worker",
                    "objective": "Produce isolated worker outputs.",
                    "context": "Merge test context.",
                    "boundaries": "Write only inside worker_workspace.",
                    "output_schema": {"type": "object"},
                    "runner_ids": runners,
                    "metadata": {
                        "isolation": {
                            "enabled": True,
                            "mode": "copy",
                            "source_root": str(source),
                            "base_dir": str(base_dir),
                        }
                    },
                }
            },
        }
    )


def _result_for_worker(workspace: Path, source: Path) -> AgentResult:
    return AgentResult(
        status="completed",
        summary="worker result",
        metadata={
            "workspace_isolated": True,
            "worker_workspace": str(workspace),
            "workspace_source_root": str(source),
        },
    )


if __name__ == "__main__":
    unittest.main()
