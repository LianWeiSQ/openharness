from __future__ import annotations

import json
import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from swarm import (
    AgentResult,
    AgentSpec,
    ArtifactRef,
    FileSwarmStateStore,
    RunContext,
    SwarmRuntime,
    Usage,
    build_function_registry,
    load_swarm_config,
)


class SwarmStateTests(unittest.IsolatedAsyncioTestCase):
    def _workspace(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = (root / f"swarm_state_{uuid4().hex}").resolve()
        path.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    async def test_file_state_store_persists_runtime_result(self) -> None:
        temp = self._workspace()
        store = FileSwarmStateStore(temp / "state")
        config = _config()

        def worker(spec: AgentSpec, ctx: RunContext) -> AgentResult:
            return AgentResult(
                status="completed",
                summary=f"done:{ctx.run_id}:{spec.role}",
                evidence=["worker.py:1"],
                open_questions=["none"],
                confidence=0.7,
                artifacts=[ArtifactRef(kind="file", uri="artifact.txt", title="Artifact", metadata={"runner": "alpha"})],
                usage=Usage(input_tokens=3, output_tokens=4, cost=0.05, steps=2, latency_ms=10),
                metadata={"custom": "value"},
            )

        result = await SwarmRuntime(
            registry=build_function_registry(config, {"alpha": worker}),
            state_store=store,
        ).run_task(config.task("task"), run_id="state-run")

        run_dir = store.run_dir("state-run")
        state_path = run_dir / "state.latest.json"
        results_path = run_dir / "runner-results.json"
        trace_path = run_dir / "trace.jsonl"
        self.assertTrue(state_path.exists())
        self.assertTrue(results_path.exists())
        self.assertTrue(trace_path.exists())

        loaded = store.load_run("state-run")
        self.assertEqual(loaded["run_id"], "state-run")
        self.assertEqual(loaded["task_id"], "task")
        self.assertEqual(loaded["status"], "completed")
        self.assertEqual(loaded["usage"]["input_tokens"], 3)
        self.assertEqual(loaded["usage"]["output_tokens"], 4)
        self.assertEqual(loaded["results"]["alpha"]["metadata"]["custom"], "value")
        self.assertEqual(loaded["results"]["alpha"]["artifacts"][0]["metadata"]["runner"], "alpha")
        self.assertEqual(loaded["results"]["alpha"]["usage"]["total_tokens"], 7)
        self.assertEqual(loaded["summary"], result.summary)

        runner_results = json.loads(results_path.read_text(encoding="utf-8"))
        self.assertEqual(runner_results["alpha"]["summary"], "done:state-run:worker")
        trace_events = [json.loads(line) for line in trace_path.read_text(encoding="utf-8").splitlines()]
        self.assertGreaterEqual(len(trace_events), 1)
        self.assertEqual(trace_events[0]["run_id"], "state-run")

    async def test_runtime_without_state_store_does_not_require_persistence(self) -> None:
        config = _config()

        def worker(_spec: AgentSpec, _ctx: RunContext) -> str:
            return "ok"

        result = await SwarmRuntime(registry=build_function_registry(config, {"alpha": worker})).run_task(config.task("task"))

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["alpha"].summary, "ok")


def _config():
    return load_swarm_config(
        {
            "runners": {
                "alpha": {"kind": "function", "roles": ["worker"]},
            },
            "tasks": {
                "task": {
                    "role": "worker",
                    "objective": "Persist swarm state.",
                    "context": "State test context.",
                    "boundaries": "Read-only.",
                    "output_schema": {"type": "object"},
                    "runner_ids": ["alpha"],
                }
            },
        }
    )


if __name__ == "__main__":
    unittest.main()
