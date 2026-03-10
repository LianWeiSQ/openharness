from __future__ import annotations

"""
用例 Demo：在现有 AgentLoop 上增加 AgentScope 外壳（ReActAgent + DashScope/Qwen）。

目标：
- OpenAgent 继续负责 AgentLoop 语义：step-start/patch/step-finish
- AgentScope 负责底层推理与工具执行（ReActAgent）
- 输出仍然是 OpenAgent 的 StreamEvent（便于 UI/CLI 统一消费）

运行（PowerShell）：
1) 安装可选依赖：
   pip install -e "openagent[agentscope]"

2) 设置阿里 DashScope Key：
   $env:DASHSCOPE_API_KEY="你的Key"
   $env:DASHSCOPE_MODEL="qwen-plus"   # 可选

3) 运行：
   python openagent/examples/run_agentscope_universal.py "请解释一下什么是幂等性？"
"""

import asyncio
import os
import sys
from pathlib import Path

# 允许直接运行本文件：把仓库根目录加入 sys.path
REPO_ROOT = Path(__file__).resolve().parents[2]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from openagent.adapter._agentscope_compat import agentscope_available  # noqa: E402
from openagent.adapter.memory_adapter import MemoryAdapter  # noqa: E402
from openagent.core.agent.agentscope_universal import AgentScopeUniversalAgent  # noqa: E402
from openagent.core.loop.processor import AgentLoop  # noqa: E402
from openagent.core.permission.manager import PermissionManager  # noqa: E402
from openagent.core.session.session import Session  # noqa: E402
from openagent.core.tool.toolkit import ToolkitAdapter  # noqa: E402
from openagent.core.types import AgentConfig, Model  # noqa: E402


async def main() -> int:
    if not agentscope_available():
        print("未安装 agentscope，可选依赖。请执行：pip install -e \"openagent[agentscope]\"")
        return 2

    if not os.getenv("DASHSCOPE_API_KEY"):
        print("未检测到环境变量 DASHSCOPE_API_KEY，请先设置后再运行。")
        return 2

    question = " ".join(sys.argv[1:]).strip()
    if not question:
        question = input("请输入你的问题：").strip()
    if not question:
        print("问题不能为空")
        return 2

    # Session 工作目录（用于快照/patch）
    workdir = Path("openagent/examples/workdir_agentscope")
    workdir.mkdir(parents=True, exist_ok=True)
    session = Session(directory=workdir)

    # 共享的权限与工具：AgentLoop 和 AgentScope 外壳共用同一套实现
    pm = PermissionManager()
    toolkit = ToolkitAdapter()
    memory = MemoryAdapter()

    # 仅用于标识模型（实际模型创建在 AgentScope backend 内完成）
    model_id = os.getenv("DASHSCOPE_MODEL", "qwen-plus")
    model = Model(
        id=model_id,
        provider_id="dashscope",
        name=f"DashScope/{model_id}",
        context_window=32768,
        max_output=2048,
    )

    agent = AgentScopeUniversalAgent(
        config=AgentConfig(
            name="agentscope-universal",
            mode="primary",
            model=model,
            # 这里给 FULL，便于 demo 测试工具能力；生产建议用更严格规则集
            permission="FULL",
            max_steps=5,
        ),
        system_prompt="你是一个专业助手，请简洁准确回答用户问题。",
        session_root=workdir,
        permission_manager=pm,
        toolkit=toolkit,
        memory=memory,
    )

    # 关键：把 toolkit 注入 AgentLoop，确保内置工具/中间件被注册到同一个 toolkit 实例
    loop = AgentLoop(agent=agent, session=session, permission_manager=pm, toolkit=toolkit)

    async for ev in loop.run(question):
        t = ev["type"]
        if t == "text-delta":
            sys.stdout.write(ev["text"])
            sys.stdout.flush()
        elif t == "tool-call":
            print(f"\n[tool-call] {ev['name']} {ev['input']}")
        elif t == "tool-result":
            if ev.get("error"):
                print(f"\n[tool-result:error] {ev['call_id']} {ev['error']}")
            else:
                print(f"\n[tool-result] {ev['call_id']} ok")
        elif t == "patch":
            print(f"\n[patch] files={len(ev.get('files') or [])}")
        elif t == "error":
            print(f"\n[error] {ev['error']}")
            return 1

    print("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))

