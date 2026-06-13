from __future__ import annotations

import ast
import unittest
from pathlib import Path

from swarm import AgentResult, RunContext, SwarmRuntime, build_function_registry, load_swarm_config
from swarm.protocol import AgentSpec, Usage


CONFIG_YAML = """
fanout_budget:
  max_concurrent: 2
  max_total_workers: 4
  max_total_tokens: 30
runners:
  researcher:
    kind: function
    roles: [research]
    handler: research_fn
    tool_groups: [read]
  reviewer:
    kind: function
    roles: [review]
    handler: review_fn
tasks:
  compare:
    role: research
    objective: Compare two implementations.
    context: Repo paths are provided in inputs.
    boundaries: Read-only. Return evidence and open questions.
    output_schema:
      type: object
      required: [summary]
    runner_ids: [researcher, reviewer]
    inputs:
      paths: ["a.py", "b.py"]
"""


class SwarmFunctionKernelTests(unittest.IsolatedAsyncioTestCase):
    async def test_yaml_config_builds_function_registry_and_runs_multiple_runners(self) -> None:
        config = load_swarm_config(CONFIG_YAML)

        def research_fn(spec: AgentSpec, _ctx: RunContext) -> dict[str, object]:
            self.assertEqual(spec.objective, "Compare two implementations.")
            self.assertEqual(spec.permissions, "READONLY")
            return {
                "summary": f"researched:{','.join(spec.inputs['paths'])}",
                "evidence": ["a.py:1"],
                "confidence": 0.8,
                "usage": {"input_tokens": 10, "output_tokens": 5, "cost": 0.01},
            }

        async def review_fn(spec: AgentSpec, _ctx: RunContext) -> AgentResult:
            self.assertEqual(spec.role, "review")
            return AgentResult(
                status="completed",
                summary="reviewed",
                open_questions=["Need runtime adapter next."],
                usage=Usage(input_tokens=4, output_tokens=3),
            )

        registry = build_function_registry(
            config,
            {
                "research_fn": research_fn,
                "review_fn": review_fn,
            },
        )
        runtime = SwarmRuntime(registry=registry, fanout_budget=config.fanout_budget)

        result = await runtime.run_task(config.task("compare"), run_id="run-test")

        self.assertEqual(result.status, "completed")
        self.assertEqual(set(result.results), {"researcher", "reviewer"})
        self.assertIn("[researcher] completed: researched:a.py,b.py", result.summary)
        self.assertIn("[reviewer] completed: reviewed", result.summary)
        self.assertEqual(result.usage.input_tokens, 14)
        self.assertEqual(result.usage.output_tokens, 8)
        self.assertFalse(result.warnings)

    async def test_runner_failure_is_captured_as_failed_result(self) -> None:
        config = load_swarm_config(
            {
                "runners": {"broken": {"kind": "function", "roles": ["worker"]}},
                "tasks": {
                    "task": {
                        "role": "worker",
                        "objective": "Run broken worker.",
                        "context": "Test context.",
                        "boundaries": "Read-only.",
                        "output_schema": {"type": "object"},
                        "runner_ids": ["broken"],
                    }
                },
            }
        )

        def broken(_spec: AgentSpec, _ctx: RunContext) -> str:
            raise RuntimeError("boom")

        registry = build_function_registry(config, {"broken": broken})
        result = await SwarmRuntime(registry=registry).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertEqual(result.results["broken"].status, "failed")
        self.assertIn("boom", result.results["broken"].summary)

    async def test_agent_spec_contract_requires_context_boundaries_and_schema(self) -> None:
        config = load_swarm_config(
            {
                "runners": {"worker": {"kind": "function", "roles": ["worker"]}},
                "tasks": {
                    "task": {
                        "role": "worker",
                        "objective": "Incomplete task.",
                        "context": "",
                        "boundaries": "",
                        "output_schema": {},
                        "runner_ids": ["worker"],
                    }
                },
            }
        )

        def worker(_spec: AgentSpec, _ctx: RunContext) -> str:
            return "should not run"

        registry = build_function_registry(config, {"worker": worker})
        result = await SwarmRuntime(registry=registry).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertIn("context", result.results["worker"].summary)
        self.assertIn("boundaries", result.results["worker"].summary)
        self.assertIn("output_schema", result.results["worker"].summary)

    def test_swarm_package_has_no_openagent_imports(self) -> None:
        root = Path(__file__).resolve().parents[1] / "swarm"
        offenders: list[str] = []
        for path in root.rglob("*.py"):
            tree = ast.parse(path.read_text(encoding="utf-8"))
            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    for alias in node.names:
                        if alias.name == "openagent" or alias.name.startswith("openagent."):
                            offenders.append(str(path))
                if isinstance(node, ast.ImportFrom) and node.module:
                    if node.module == "openagent" or node.module.startswith("openagent."):
                        offenders.append(str(path))
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main()
