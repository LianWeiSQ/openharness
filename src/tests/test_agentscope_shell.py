from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.adapter.agentscope_adapter import AgentScopeBackend, AgentScopeBackendResult
from openagent.adapter.memory_adapter import MemoryAdapter
from openagent.core.agent.agentscope_universal import AgentScopeUniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.session.session import Session
from openagent.core.tool.toolkit import ToolkitAdapter
from openagent.core.types import AgentConfig, Model, Usage


class _FakeBackend(AgentScopeBackend):
    """
    不依赖真实 agentscope 的 fake backend：
    - 模拟一次 write 工具调用（通过 OpenAgent ToolkitAdapter 执行）
    - 输出一段文本
    """

    def __init__(self, *, toolkit: ToolkitAdapter, session_root: Path, memory: MemoryAdapter) -> None:
        self.toolkit = toolkit
        self.session_root = session_root
        self.memory = memory

    async def run(self, *, system, messages, sink):
        call_id = "fake_c1"
        sink.tool_call(name="write", input={"file_path": "a.txt", "content": "hi"}, call_id=call_id)
        result = await self.toolkit.execute(
            name="write",
            input={"file_path": "a.txt", "content": "hi"},
            call_id=call_id,
            context={"session_root": str(self.session_root), "memory": self.memory},
        )
        sink.tool_result(call_id=call_id, output=result.output, error=result.error)
        sink.text("done")
        return AgentScopeBackendResult(text="done", usage=Usage())


class AgentScopeShellTests(unittest.IsolatedAsyncioTestCase):
    async def test_loop_does_not_double_execute_tools(self) -> None:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"t_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        try:
            # 共享对象
            pm = PermissionManager()
            toolkit = ToolkitAdapter()
            memory = MemoryAdapter()

            # 计数：如果 AgentLoop 发生“二次执行”，execute 会被调用超过 1 次
            execute_calls = {"n": 0}
            orig_execute = toolkit.execute

            async def _counting_execute(**kwargs):
                execute_calls["n"] += 1
                return await orig_execute(**kwargs)

            toolkit.execute = _counting_execute  # type: ignore[assignment]

            backend = _FakeBackend(toolkit=toolkit, session_root=td, memory=memory)
            agent = AgentScopeUniversalAgent(
                config=AgentConfig(
                    name="agentscope-test",
                    permission="FULL",
                    max_steps=3,
                    model=Model(id="qwen-plus", provider_id="dashscope", name="x", context_window=1, max_output=1),
                ),
                system_prompt="",
                session_root=td,
                permission_manager=pm,
                toolkit=toolkit,
                memory=memory,
                backend=backend,
            )

            session = Session(directory=td)
            loop = AgentLoop(agent=agent, session=session, permission_manager=pm, toolkit=toolkit)
            events = []
            async for ev in loop.run("hi"):
                events.append(ev)

            # fake backend 会调用一次 write；AgentLoop 不应再次调用
            self.assertEqual(execute_calls["n"], 1)
            self.assertTrue((td / "a.txt").exists())
            self.assertIn("tool-result", [e["type"] for e in events])
        finally:
            shutil.rmtree(td, ignore_errors=True)

