from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.session.session import Session
from openagent.core.types import AgentConfig, Model

from _mock_model import ScriptedLanguageModel


def _make_model_metadata(*, context_window: int = 256, max_output: int = 64) -> Model:
    return Model(
        id="test-model",
        provider_id="test",
        name="Test Model",
        context_window=context_window,
        max_output=max_output,
    )


def _success_script() -> list[list[dict[str, object]]]:
    return [
        [
            {"type": "text-delta", "id": "t1", "text": "done"},
            {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
        ]
    ]


class LoopTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_dir(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"t_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, td, True)
        return td

    async def test_loop_executes_tool_and_emits_patch(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "tool-call", "call_id": "c1", "name": "write", "input": {"file_path": "a.txt", "content": "hi"}},
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for ev in loop.run("hi"):
            events.append(ev)

        types = [e["type"] for e in events]
        self.assertIn("tool-result", types)
        self.assertIn("patch", types)

    async def test_loop_errors_before_provider_call_when_context_budget_overflows(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            model=_make_model_metadata(context_window=96, max_output=24),
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for ev in loop.run("x" * 800):
            events.append(ev)

        self.assertEqual(model.call_index, 0)
        self.assertEqual(events[-1]["type"], "error")
        self.assertIn("Context budget exceeded before model call", events[-1]["error"])
        self.assertIn("estimated_input_tokens=", events[-1]["error"])

    async def test_loop_skips_context_budget_when_disabled(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            model=_make_model_metadata(context_window=96, max_output=24),
            options={"context_budget": {"enabled": False}},
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for ev in loop.run("x" * 800):
            events.append(ev)

        self.assertEqual(model.call_index, 1)
        self.assertIn("text-delta", [e["type"] for e in events])

    async def test_loop_skips_context_budget_without_model_metadata(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(name="u", permission="FULL", max_steps=5, model=None)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for ev in loop.run("x" * 800):
            events.append(ev)

        self.assertEqual(model.call_index, 1)
        self.assertIn("text-delta", [e["type"] for e in events])

    async def test_loop_skips_context_budget_with_non_positive_context_window(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            model=_make_model_metadata(context_window=0, max_output=24),
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for ev in loop.run("x" * 800):
            events.append(ev)

        self.assertEqual(model.call_index, 1)
        self.assertIn("text-delta", [e["type"] for e in events])
