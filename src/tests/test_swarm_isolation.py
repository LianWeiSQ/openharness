from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from swarm import AgentResult, AgentSpec, RunContext, SwarmRuntime, build_function_registry, load_swarm_config


class SwarmIsolationTests(unittest.IsolatedAsyncioTestCase):
    def _workspace(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = (root / f"swarm_isolation_{uuid4().hex}").resolve()
        path.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    async def test_copy_isolation_gives_each_runner_a_distinct_workspace(self) -> None:
        temp = self._workspace()
        source = temp / "source"
        source.mkdir()
        (source / "file.txt").write_text("source", encoding="utf-8")
        base_dir = temp / "isolated"
        seen: dict[str, Path] = {}

        def worker(spec: AgentSpec, ctx: RunContext) -> AgentResult:
            runner_id = str(spec.metadata["runner_id"])
            workspace = Path(str(spec.inputs["worker_workspace"]))
            self.assertEqual(ctx.metadata["worker_workspace"], str(workspace))
            self.assertTrue((workspace / "file.txt").exists())
            (workspace / f"{runner_id}.txt").write_text("worker output", encoding="utf-8")
            seen[runner_id] = workspace
            return AgentResult(status="completed", summary=f"done:{runner_id}")

        config = load_swarm_config(
            {
                "fanout_budget": {"max_concurrent": 2},
                "runners": {
                    "alpha": {"kind": "function", "roles": ["worker"]},
                    "beta": {"kind": "function", "roles": ["worker"]},
                },
                "tasks": {
                    "task": {
                        "role": "worker",
                        "objective": "Use isolated workspaces.",
                        "context": "Isolation test context.",
                        "boundaries": "Write only inside worker_workspace.",
                        "output_schema": {"type": "object"},
                        "runner_ids": ["alpha", "beta"],
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

        result = await SwarmRuntime(registry=build_function_registry(config, {"alpha": worker, "beta": worker})).run_task(
            config.task("task"),
            run_id="isolation-run",
        )

        self.assertEqual(result.status, "completed")
        self.assertEqual(set(seen), {"alpha", "beta"})
        self.assertNotEqual(seen["alpha"], seen["beta"])
        self.assertFalse((source / "alpha.txt").exists())
        self.assertFalse((source / "beta.txt").exists())
        finished = [event for event in result.trace_events if event.name == "swarm.runner.finished"]
        self.assertEqual(len(finished), 2)
        self.assertTrue(all(event.attributes["workspace_isolated"] for event in finished))
        self.assertEqual({event.attributes["workspace_mode"] for event in finished}, {"copy"})

    async def test_runner_isolation_overrides_task_default(self) -> None:
        temp = self._workspace()
        source = temp / "source"
        source.mkdir()
        (source / "file.txt").write_text("source", encoding="utf-8")
        base_dir = temp / "isolated"
        seen_workspace: list[Path] = []

        def worker(spec: AgentSpec, _ctx: RunContext) -> str:
            workspace = Path(str(spec.inputs["worker_workspace"]))
            seen_workspace.append(workspace)
            self.assertFalse((workspace / "file.txt").exists())
            return "empty workspace"

        config = load_swarm_config(
            {
                "runners": {
                    "empty_worker": {
                        "kind": "function",
                        "roles": ["worker"],
                        "metadata": {
                            "isolation": {
                                "enabled": True,
                                "mode": "empty",
                                "base_dir": str(base_dir),
                            }
                        },
                    }
                },
                "tasks": {
                    "task": {
                        "role": "worker",
                        "objective": "Use runner override.",
                        "context": "Isolation test context.",
                        "boundaries": "Write only inside worker_workspace.",
                        "output_schema": {"type": "object"},
                        "runner_ids": ["empty_worker"],
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

        result = await SwarmRuntime(registry=build_function_registry(config, {"empty_worker": worker})).run_task(config.task("task"))

        self.assertEqual(result.status, "completed")
        self.assertEqual(len(seen_workspace), 1)
        self.assertTrue(seen_workspace[0].exists())
        finished = next(event for event in result.trace_events if event.name == "swarm.runner.finished")
        self.assertEqual(finished.attributes["workspace_mode"], "empty")

    async def test_isolation_is_opt_in(self) -> None:
        config = load_swarm_config(
            {
                "runners": {"worker": {"kind": "function", "roles": ["worker"]}},
                "tasks": {
                    "task": {
                        "role": "worker",
                        "objective": "Run without isolation.",
                        "context": "No workspace needed.",
                        "boundaries": "Read-only.",
                        "output_schema": {"type": "object"},
                        "runner_ids": ["worker"],
                    }
                },
            }
        )

        def worker(spec: AgentSpec, ctx: RunContext) -> str:
            self.assertNotIn("worker_workspace", spec.inputs)
            self.assertNotIn("worker_workspace", spec.metadata)
            self.assertNotIn("worker_workspace", ctx.metadata)
            return "ok"

        result = await SwarmRuntime(registry=build_function_registry(config, {"worker": worker})).run_task(config.task("task"))

        self.assertEqual(result.status, "completed")

    async def test_invalid_isolation_config_is_captured_as_runner_failure(self) -> None:
        config = load_swarm_config(
            {
                "runners": {"worker": {"kind": "function", "roles": ["worker"]}},
                "tasks": {
                    "task": {
                        "role": "worker",
                        "objective": "Run with bad isolation.",
                        "context": "Bad config should not crash the supervisor.",
                        "boundaries": "Read-only.",
                        "output_schema": {"type": "object"},
                        "runner_ids": ["worker"],
                        "metadata": {"isolation": {"enabled": True, "mode": "unknown"}},
                    }
                },
            }
        )

        def worker(_spec: AgentSpec, _ctx: RunContext) -> str:
            return "should not run"

        result = await SwarmRuntime(registry=build_function_registry(config, {"worker": worker})).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertEqual(result.results["worker"].status, "failed")
        self.assertIn("unsupported workspace isolation mode", result.results["worker"].summary)
        finished = next(event for event in result.trace_events if event.name == "swarm.runner.finished")
        self.assertEqual(finished.status, "error")


if __name__ == "__main__":
    unittest.main()
