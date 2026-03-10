from __future__ import annotations

"""
用例 Demo：调用 UniversalAgent + 阿里云 DashScope（通义千问）进行问答。

运行方式（推荐在仓库根目录执行）：

1) 设置 API Key（Windows PowerShell）：
   $env:DASHSCOPE_API_KEY="你的Key"

2) 运行：
   python openagent/examples/run_dashscope_universal.py "给我一句关于软件工程的建议"

说明：
- 该 demo 走 DashScope 的 OpenAI 兼容接口（compatible-mode）
- 为了避免“模型自动调用工具”带来额外环境依赖，本 demo 不向模型传 tools
"""

import asyncio
import os
import sys
from pathlib import Path

# 允许直接运行本文件：把仓库根目录加入 sys.path
# 说明：sys.path 里应当放“包目录的父目录”，而不是包目录本身。
# 我们通过向上查找 `Agent.md` 来定位仓库根目录。
def _find_repo_root() -> Path:
    here = Path(__file__).resolve()
    for p in here.parents:
        if (p / "Agent.md").exists() and (p / "openagent").exists():
            return p
    # 兜底：按当前文件路径层级猜测（openagent/src/examples/ -> repo root）
    return here.parents[3]


REPO_ROOT = _find_repo_root()
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from openagent.core.agent.universal import UniversalAgent  # noqa: E402
from openagent.core.loop.processor import AgentLoop  # noqa: E402
from openagent.core.permission.manager import PermissionManager  # noqa: E402
from openagent.core.provider.dashscope import DashScopeProvider  # noqa: E402
from openagent.core.session.session import Session  # noqa: E402
from openagent.core.types import AgentConfig, ChatMessage, Model  # noqa: E402


async def main() -> int:
    # 读取提问内容：优先命令行参数，否则从 stdin 读一行
    question = " ".join(sys.argv[1:]).strip()
    if not question:
        question = input("请输入你的问题：").strip()
    if not question:
        print("问题不能为空")
        return 2

    # 检查 API Key
    if not os.getenv("DASHSCOPE_API_KEY"):
        print("未检测到环境变量 DASHSCOPE_API_KEY，请先设置后再运行。")
        return 2

    # 选择模型（可通过环境变量覆盖）
    model_id = os.getenv("DASHSCOPE_MODEL", "qwen3.5-plus")
    # Model 结构用于给 AgentLoop/Provider 传递“模型身份与限制信息”
    model = Model(
        id=model_id,
        provider_id="dashscope",
        name=f"DashScope/{model_id}",
        context_window=32768,
        max_output=2048,
    )

    # 初始化 Provider 并获取语言模型适配器
    provider = DashScopeProvider()
    language_model = await provider.get_language_model(model)

    # 为纯问答场景配置一个 UniversalAgent
    # 注意：这里刻意用更“安全”的权限/工具配置，避免触发文件/命令等行为
    agent = UniversalAgent(
        config=AgentConfig(
            name="universal-dashscope",
            mode="primary",
            prompt=None,
            model=model,
            tools="readonly",
            permission="NONE",
            max_steps=5,
            temperature=float(os.getenv("DASHSCOPE_TEMPERATURE", "0.2")),
        ),
        model=language_model,
        system_prompt=(
            "你是一个专业助手，请直接回答用户问题。"
            "除非用户明确要求，否则不要提出执行命令/读写文件等操作。"
        ),
    )

    # 创建会话工作目录（用于快照/patch；纯问答一般不会产生文件变更）
    workdir = Path("openagent/examples/workdir_dashscope")
    workdir.mkdir(parents=True, exist_ok=True)
    session = Session(directory=workdir)

    # 权限管理器：demo 默认不弹窗询问；若要 ask/allow 机制可自行注入 ask_user_func
    pm = PermissionManager()

    loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

    # 执行并打印“文本流事件”
    print(f"模型：{model_id}")
    print("回答：")
    async for ev in loop.run(question):
        if ev["type"] == "text-delta":
            # 逐段输出（本实现是一次性输出，但上层仍按流式处理）
            sys.stdout.write(ev["text"])
            sys.stdout.flush()
        if ev["type"] == "error":
            print(f"\n[错误] {ev['error']}")
            return 1
    print("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
