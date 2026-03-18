from __future__ import annotations

import shutil
import unittest
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.session.session import Session
from openagent.core.tool.definition import ToolContext, ToolOutput
from openagent.core.tool.toolkit import ToolkitAdapter
from openagent.core.types import AgentConfig

from _mock_model import ScriptedLanguageModel


@dataclass
class NoArgs:
    pass


class ToolPolicyTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_dir(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"t_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, td, True)
        return td

    def _make_loop(self, *, model: ScriptedLanguageModel, tools: list[str] | str) -> tuple[AgentLoop, Session]:
        cfg = AgentConfig(name="u", permission="FULL", tools=tools, max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="")
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=PermissionManager(), toolkit=ToolkitAdapter())
        return loop, session

    def _override_simple_tool(self, loop: AgentLoop, tool_name: str) -> None:
        @loop.toolkit.registry.define_tool(tool_id=tool_name, parameters=NoArgs, description=f"# {tool_name}", group="test")
        async def _tool(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
            return ToolOutput(title=f"{tool_name} ok", output=f"{tool_name} result")

    async def test_research_requests_retry_until_web_search_is_called(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "This answer comes only from memory."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "tool-call", "call_id": "w1", "name": "web_search", "input": {"query": "new material photoelectric conversion efficiency"}},
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t2", "text": "Final answer with research evidence."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        loop, session = self._make_loop(model=model, tools=["web_search"])
        self._override_simple_tool(loop, "web_search")

        events = []
        async for event in loop.run("请研究一种新型材料的光电转化效率"):
            events.append(event)

        self.assertEqual(model.call_index, 3)
        self.assertIn("tool-call", [event["type"] for event in events])
        self.assertIn("tool-result", [event["type"] for event in events])
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "stop")
        self.assertFalse(any(event.get("type") == "text-delta" and "memory" in event.get("text", "") for event in events))
        self.assertFalse(any(message.role == "assistant" and "memory" in message.content for message in session.messages))

    async def test_current_requests_error_before_provider_call_when_web_search_is_unavailable(self) -> None:
        model = ScriptedLanguageModel(script=[])
        loop, _session = self._make_loop(model=model, tools=[])

        events = []
        async for event in loop.run("请告诉我今天最新的光伏政策"):
            events.append(event)

        self.assertEqual(model.call_index, 0)
        self.assertEqual(events[-1]["type"], "error")
        self.assertIn("scenario=current", events[-1]["error"])
        self.assertIn("missing=web_search", events[-1]["error"])

    async def test_plan_requests_error_before_provider_call_when_todo_tools_are_unavailable(self) -> None:
        model = ScriptedLanguageModel(script=[])
        loop, _session = self._make_loop(model=model, tools=["web_search"])
        self._override_simple_tool(loop, "web_search")

        events = []
        async for event in loop.run("请帮我设计一套完整的实验方案"):
            events.append(event)

        self.assertEqual(model.call_index, 0)
        self.assertEqual(events[-1]["type"], "error")
        self.assertIn("scenario=plan", events[-1]["error"])
        self.assertIn("missing=todoread, todowrite", events[-1]["error"])

    async def test_explicit_system_prompt_bypasses_default_tool_policy_guard(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "Answer without tools."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ]
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", tools=[], max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Custom system prompt.")
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=PermissionManager(), toolkit=ToolkitAdapter())

        events = []
        async for event in loop.run("请研究一种新型材料的光电转化效率"):
            events.append(event)

        self.assertEqual(model.call_index, 1)
        self.assertNotIn("error", [event["type"] for event in events])
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "stop")
