from __future__ import annotations

import ast
import shutil
import unittest
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

from openagent.integrations.swarm import OpenAgentRunner, build_openagent_registry
from swarm import AgentResult, AgentSpec, RunContext, SwarmRuntime
from swarm.config import RunnerConfig, TaskConfig, load_swarm_config
from swarm.registry import RunnerRegistry

from _mock_model import ScriptedLanguageModel
from test_loop import _make_model_metadata


class RaisingModel:
    async def stream(self, **_kwargs):
        raise RuntimeError("model exploded")
        yield {}  # pragma: no cover


@dataclass
class TempWorkspace:
    path: Path

    def cleanup(self) -> None:
        shutil.rmtree(self.path, ignore_errors=True)


class OpenAgentSwarmAdapterTests(unittest.IsolatedAsyncioTestCase):
    def _workspace(self) -> TempWorkspace:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        path = (tmp_root / f"swarm_{uuid4().hex}").resolve()
        path.mkdir(parents=True, exist_ok=True)
        workspace = TempWorkspace(path)
        self.addCleanup(workspace.cleanup)
        return workspace

    def _spec(self) -> AgentSpec:
        return AgentSpec(
            role="research",
            objective="Summarize the workspace.",
            context="The workspace contains files to inspect.",
            boundaries="Read-only. Do not modify files.",
            output_schema={"type": "object", "required": ["summary"]},
        )

    async def test_openagent_runner_returns_agent_result_from_loop(self) -> None:
        workspace = self._workspace()
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "worker summary"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 7, "output_tokens": 3, "cost": 0.02}},
                ]
            ]
        )
        runner = OpenAgentRunner(
            runner_id="oa-researcher",
            roles=["research"],
            model=model,
            model_metadata=_make_model_metadata(context_window=32768, max_output=256),
            workspace_root=workspace.path,
        )

        handle = await runner.start(self._spec(), RunContext(run_id="swarm-run"))
        result = await handle.result()
        events = [event async for event in handle.events()]

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.summary, "worker summary")
        self.assertEqual(result.usage.input_tokens, 7)
        self.assertEqual(result.usage.output_tokens, 3)
        self.assertEqual(result.metadata["runner_id"], "oa-researcher")
        self.assertEqual(events[0].type, "runner.started")
        self.assertEqual(events[-1].type, "runner.finished")
        self.assertEqual(model.seen_tools_by_call[0], ["read", "glob", "grep", "ls", "skill", "todoread", "question"])
        rendered_messages = "\n".join(str(message.content) for message in model.seen_messages_by_call[0])
        self.assertIn("Swarm worker contract", rendered_messages)
        self.assertIn("Summarize the workspace", rendered_messages)

    async def test_openagent_runner_can_execute_readonly_tool_call(self) -> None:
        workspace = self._workspace()
        (workspace.path / "notes.txt").write_text("hello swarm", encoding="utf-8")
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "tool-call", "call_id": "call-ls", "name": "ls", "input": {"path": "."}},
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 5, "output_tokens": 2, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t2", "text": "saw notes.txt"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 6, "output_tokens": 4, "cost": 0.01}},
                ],
            ]
        )
        runner = OpenAgentRunner(
            runner_id="oa-reader",
            roles=["research"],
            model=model,
            model_metadata=_make_model_metadata(context_window=32768, max_output=256),
            workspace_root=workspace.path,
        )

        result = await (await runner.start(self._spec(), RunContext(run_id="swarm-run"))).result()

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.summary, "saw notes.txt")
        self.assertEqual(result.metadata["tool_call_count"], 1)
        self.assertEqual(result.usage.steps, 2)
        self.assertEqual(result.usage.input_tokens, 11)
        self.assertEqual(result.usage.output_tokens, 6)

    async def test_openagent_runner_failure_is_captured(self) -> None:
        workspace = self._workspace()
        runner = OpenAgentRunner(
            runner_id="oa-broken",
            roles=["research"],
            model=RaisingModel(),
            model_metadata=_make_model_metadata(context_window=32768, max_output=256),
            workspace_root=workspace.path,
        )

        result = await (await runner.start(self._spec(), RunContext(run_id="swarm-run"))).result()

        self.assertEqual(result.status, "failed")
        self.assertIn("model exploded", result.summary)

    async def test_openagent_runner_from_config_works_in_swarm_runtime(self) -> None:
        workspace = self._workspace()
        model_a = ScriptedLanguageModel(
            script=[[{"type": "text-delta", "id": "a", "text": "A"}, {"type": "finish", "finish_reason": "stop", "usage": {}}]]
        )
        model_b = ScriptedLanguageModel(
            script=[[{"type": "text-delta", "id": "b", "text": "B"}, {"type": "finish", "finish_reason": "stop", "usage": {}}]]
        )
        registry = RunnerRegistry()
        registry.register(
            OpenAgentRunner.from_config(
                RunnerConfig(id="oa-a", kind="openagent", roles=["research"], metadata={"tools": "readonly"}),
                model=model_a,
                model_metadata=_make_model_metadata(context_window=32768, max_output=128),
                workspace_root=workspace.path,
            )
        )
        registry.register(
            OpenAgentRunner.from_config(
                RunnerConfig(id="oa-b", kind="openagent", roles=["research"], metadata={"tools": "readonly"}),
                model=model_b,
                model_metadata=_make_model_metadata(context_window=32768, max_output=128),
                workspace_root=workspace.path,
            )
        )
        task = TaskConfig(
            id="fanout",
            role="research",
            objective="Run both workers.",
            context="Test context.",
            boundaries="Read-only.",
            output_schema={"type": "object"},
            runner_ids=["oa-a", "oa-b"],
        )

        result = await SwarmRuntime(registry=registry).run_task(task, run_id="swarm-run")

        self.assertEqual(result.status, "completed")
        self.assertEqual({runner_id: item.summary for runner_id, item in result.results.items()}, {"oa-a": "A", "oa-b": "B"})

    async def test_build_openagent_registry_from_yaml_config(self) -> None:
        workspace = self._workspace()
        config = load_swarm_config(
            {
                "runners": {
                    "oa-reader": {
                        "kind": "openagent",
                        "roles": ["research"],
                        "metadata": {"tools": "readonly", "example": "mixed"},
                    },
                    "remote-reviewer": {
                        "kind": "a2a",
                        "roles": ["research"],
                        "metadata": {"url": "http://127.0.0.1:9/a2a"},
                    },
                },
                "tasks": {
                    "mixed": {
                        "role": "research",
                        "objective": "Run OpenAgent only for this test.",
                        "context": "The A2A runner is configured but not registered here.",
                        "boundaries": "Read-only.",
                        "output_schema": {"type": "object"},
                        "runner_ids": ["oa-reader"],
                    }
                },
            }
        )
        model = ScriptedLanguageModel(
            script=[[{"type": "text-delta", "id": "m", "text": "yaml builder ok"}, {"type": "finish", "finish_reason": "stop", "usage": {}}]]
        )

        registry = build_openagent_registry(
            config,
            model=model,
            model_metadata=_make_model_metadata(context_window=32768, max_output=128),
            workspace_root=workspace.path,
        )
        result = await SwarmRuntime(registry=registry).run_task(config.task("mixed"), run_id="mixed-yaml")

        self.assertEqual(list(registry.ids()), ["oa-reader"])
        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["oa-reader"].summary, "yaml builder ok")

    def test_swarm_package_still_has_no_openagent_imports(self) -> None:
        root = Path(__file__).resolve().parents[1] / "swarm"
        offenders: list[str] = []
        for path in root.rglob("*.py"):
            tree = ast.parse(path.read_text(encoding="utf-8"))
            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    offenders.extend(alias.name for alias in node.names if alias.name == "openagent" or alias.name.startswith("openagent."))
                if isinstance(node, ast.ImportFrom) and node.module:
                    if node.module == "openagent" or node.module.startswith("openagent."):
                        offenders.append(node.module)
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main()
