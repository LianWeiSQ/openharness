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
from openagent.core.types import AgentConfig


@dataclass(slots=True)
class CapturingModel:
    """
    一个用于测试的 LanguageModel：
    - 记录 AgentLoop 传入的 tools 列表
    - 不输出任何文本，只直接 finish
    """

    seen_tools: list[str] | None = None

    async def stream(self, *, system, messages, tools, temperature=None, max_output_tokens=None, options=None):
        self.seen_tools = [t.name for t in tools]
        yield {"type": "finish", "finish_reason": "stop", "usage": {}}


class ToolFilterTests(unittest.IsolatedAsyncioTestCase):
    async def test_tools_readonly_exposes_only_file_read_tools(self) -> None:
        model = CapturingModel()
        cfg = AgentConfig(name="u", permission="FULL", tools="readonly", max_steps=1)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="")
        pm = PermissionManager()
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"t_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        try:
            session = Session(directory=Path(td))
            loop = AgentLoop(agent=agent, session=session, permission_manager=pm)
            async for _ev in loop.run("hi"):
                pass
        finally:
            shutil.rmtree(td, ignore_errors=True)

        self.assertIsNotNone(model.seen_tools)
        self.assertEqual(set(model.seen_tools or []), {"read", "glob", "grep", "ls"})

    async def test_permission_none_exposes_no_tools(self) -> None:
        model = CapturingModel()
        cfg = AgentConfig(name="u", permission="NONE", tools="all", max_steps=1)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="")
        pm = PermissionManager()
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"t_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        try:
            session = Session(directory=Path(td))
            loop = AgentLoop(agent=agent, session=session, permission_manager=pm)
            async for _ev in loop.run("hi"):
                pass
        finally:
            shutil.rmtree(td, ignore_errors=True)

        self.assertIsNotNone(model.seen_tools)
        self.assertEqual(model.seen_tools, [])

