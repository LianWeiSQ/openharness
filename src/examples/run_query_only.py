from __future__ import annotations

"""
Run a pure question-answer query through the current OpenAgent stack.

Behavior:
- If `DASHSCOPE_API_KEY` is present, use the real DashScope provider.
- Otherwise fall back to a local scripted model so the end-to-end query path
  can still be exercised without network credentials.

Usage:
    python openagent/src/examples/run_query_only.py "你的问题"
"""

import asyncio
import os
import sys
from pathlib import Path


def _find_repo_root() -> Path:
    here = Path(__file__).resolve()
    for p in here.parents:
        if (p / "pyproject.toml").exists() and (p / "src" / "openagent").is_dir():
            return p
    return here.parents[3]


REPO_ROOT = _find_repo_root()
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from openagent.core.agent.universal import UniversalAgent  # noqa: E402
from openagent.core.loop.processor import AgentLoop  # noqa: E402
from openagent.core.permission.manager import PermissionManager  # noqa: E402
from openagent.core.provider.dashscope import DashScopeProvider  # noqa: E402
from openagent.core.session.session import Session  # noqa: E402
from openagent.core.types import AgentConfig, Model  # noqa: E402


FALLBACK_QUERY = (
    "阐述地球气候系统中三个已识别的潜在临界点（Tipping Points），"
    "解释其触发机制、相互之间的正反馈关联，以及对全球气候系统的级联影响。"
)

FALLBACK_ANSWER = """三个经常被讨论、且具有潜在全球级联效应的气候临界点是：格陵兰冰盖、大西洋经向翻转环流（AMOC）和亚马孙雨林。

1. 格陵兰冰盖：触发机制主要是高纬增温导致表面融化增强。冰面降低后会暴露更暗的地表，反照率下降，吸收更多太阳辐射，形成“融化越多、升温越快”的正反馈。其长期后果是海平面持续上升，并向北大西洋输入更多淡水。

2. AMOC：这是把热量从低纬输送到北大西洋的重要海洋环流。其脆弱点在于北大西洋表层水必须足够咸、足够冷，才能下沉并维持翻转。当格陵兰融水和高纬降水增加时，表层盐度下降，海水更难下沉，环流就可能减弱甚至跨过临界阈值。AMOC 一旦减弱，会重排热量分布，改变欧洲、热带大西洋和季风系统。

3. 亚马孙雨林：触发机制来自增温、干旱、森林砍伐和火灾的共同作用。雨林依赖强蒸散维持区域降水；当森林退化到一定程度，蒸散减弱，降水减少，旱季变长，火灾更多，进而导致更多森林死亡，形成“干旱-火灾-退化”的自增强反馈，可能使部分区域转向稀树草原化。

这三者之间存在可串联的正反馈。格陵兰融水可削弱 AMOC；AMOC 减弱又会改变热带降水带和海温分布，影响南美季风与亚马孙降水；亚马孙退化会释放大量碳并削弱陆地碳汇，进一步抬高全球温度，从而反过来加速格陵兰融化，并增加高纬海冰和海洋环流系统的不稳定性。

级联影响在全球尺度上可能表现为：海平面长期上升、北大西洋和热带降水格局重排、极端天气与热浪风险上升、生态系统碳汇能力下降，以及更多区域性临界点被推近。也就是说，单个临界点不是孤立事件，而可能构成一个彼此放大的风险网络。"""


class ScriptedAnswerModel:
    """Local fallback model for smoke-testing the query path."""

    def __init__(self, answer: str) -> None:
        self._answer = answer

    async def stream(
        self,
        *,
        system,
        messages,
        tools,
        temperature=None,
        max_output_tokens=None,
        options=None,
    ):
        del system, messages, tools, temperature, max_output_tokens, options

        for chunk in self._answer.splitlines(keepends=True):
            yield {"type": "text-delta", "text": chunk}
        yield {"type": "finish", "finish_reason": "stop", "usage": {}}


async def _build_agent(question: str) -> tuple[UniversalAgent, str]:
    if os.getenv("DASHSCOPE_API_KEY"):
        model_id = os.getenv("DASHSCOPE_MODEL", "qwen3.5-plus")
        model = Model(
            id=model_id,
            provider_id="dashscope",
            name=f"DashScope/{model_id}",
            context_window=32768,
            max_output=2048,
        )
        provider = DashScopeProvider()
        language_model = await provider.get_language_model(model)
        return (
            UniversalAgent(
                config=AgentConfig(
                    name="query-only-dashscope",
                    mode="primary",
                    prompt=None,
                    model=model,
                    tools=[],
                    permission="NONE",
                    max_steps=3,
                    temperature=float(os.getenv("DASHSCOPE_TEMPERATURE", "0.2")),
                    options={
                        "stream": os.getenv("DASHSCOPE_STREAM", "1").lower() not in ("0", "false", "no"),
                    },
                ),
                model=language_model,
                system_prompt="你是一个专业助手，请直接回答用户问题。当前任务是纯问答，不要调用工具。",
            ),
            "dashscope",
        )

    fallback_text = FALLBACK_ANSWER if question.strip() == FALLBACK_QUERY else (
        "当前未检测到 DASHSCOPE_API_KEY，所以本次运行使用本地 scripted model 做链路演示。\n"
        "这说明 OpenAgent 当前架构可以成功跑通纯 query 的执行链；"
        "若你想得到真实模型回答，请先设置 DASHSCOPE_API_KEY 后重跑同一命令。"
    )
    model = Model(
        id="scripted-answer",
        provider_id="local",
        name="Local/ScriptedAnswer",
        context_window=8192,
        max_output=2048,
    )
    return (
        UniversalAgent(
            config=AgentConfig(
                name="query-only-local",
                mode="primary",
                prompt=None,
                model=model,
                tools=[],
                permission="NONE",
                max_steps=1,
                temperature=0.0,
            ),
            model=ScriptedAnswerModel(fallback_text),
            system_prompt="你是一个本地演示模型，用于验证 OpenAgent 的纯问答链路。",
        ),
        "local-scripted",
    )


async def main() -> int:
    question = " ".join(sys.argv[1:]).strip()
    if not question:
        question = input("请输入你的问题：").strip()
    if not question:
        print("问题不能为空")
        return 2

    agent, mode = await _build_agent(question)
    workdir = Path("examples/workdir_query_only")
    workdir.mkdir(parents=True, exist_ok=True)

    loop = AgentLoop(
        agent=agent,
        session=Session(directory=workdir),
        permission_manager=PermissionManager(),
    )

    print(f"mode={mode}")
    if mode == "local-scripted":
        print("note=未检测到 DASHSCOPE_API_KEY，当前为本地 smoke demo。")
    print("answer:")

    async for ev in loop.run(question):
        if ev["type"] == "text-delta":
            sys.stdout.write(ev["text"])
            sys.stdout.flush()
        elif ev["type"] == "tool-call":
            print(f"\n[tool-call] {ev['name']} {ev['input']}")
        elif ev["type"] == "tool-result":
            err = f" error={ev['error']}" if ev.get("error") else ""
            print(f"\n[tool-result] call_id={ev['call_id']}{err}\n{ev['output']}")
        elif ev["type"] == "error":
            print(f"\n[error] {ev['error']}")
            return 1

    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
