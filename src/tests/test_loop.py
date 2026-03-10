from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.session.session import Session
from openagent.core.types import AgentConfig

from _mock_model import ScriptedLanguageModel


class LoopTests(unittest.IsolatedAsyncioTestCase):
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
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"t_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        try:
            session = Session(directory=Path(td))
            loop = AgentLoop(agent=agent, session=session, permission_manager=pm)
            events = []
            async for ev in loop.run("hi"):
                events.append(ev)
        finally:
            shutil.rmtree(td, ignore_errors=True)
        types = [e["type"] for e in events]
        self.assertIn("tool-result", types)
        self.assertIn("patch", types)
