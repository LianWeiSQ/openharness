from __future__ import annotations

"""
Run a pure question-answer query through the current OpenAgent stack.

Behavior:
- Requires `OPENAI_API_KEY` and uses the OpenAI-compatible provider.
- For local Sub2API, source `scripts/local/use-sub2api.sh` first.

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
from openagent.core.provider.openai import OpenAIProvider  # noqa: E402
from openagent.core.session.session import Session  # noqa: E402
from openagent.core.types import AgentConfig, Model  # noqa: E402


FALLBACK_QUERY = (
    "阐述地球气候系统中三个已识别的潜在临界点（Tipping Points），"
    "解释其触发机制、相互之间的正反馈关联，以及对全球气候系统的级联影响。"
)

async def _build_agent(question: str) -> tuple[UniversalAgent, str]:
    if not os.getenv("OPENAI_API_KEY"):
        raise RuntimeError(
            "Missing OPENAI_API_KEY. For local Sub2API, run: source scripts/local/use-sub2api.sh"
        )
    model_id = os.getenv("OPENAI_MODEL", "gpt-4o-mini")
    model = Model(
        id=model_id,
        provider_id="openai",
        name=f"OpenAI Compatible/{model_id}",
        context_window=_env_int("OPENAI_CONTEXT_WINDOW", 32768),
        max_output=_env_int("OPENAI_MAX_OUTPUT", 2048),
    )
    provider = OpenAIProvider()
    language_model = await provider.get_language_model(model)
    return (
        UniversalAgent(
            config=AgentConfig(
                name="query-only-openai",
                mode="primary",
                prompt=None,
                model=model,
                tools=[],
                permission="NONE",
                max_steps=3,
                temperature=_env_float("OPENAI_TEMPERATURE", 0.2),
                options={"stream": os.getenv("OPENAI_STREAM", "1").lower() not in ("0", "false", "no")},
            ),
            model=language_model,
            system_prompt="你是一个专业助手，请直接回答用户问题。当前任务是纯问答，不要调用工具。",
        ),
        "openai-compatible",
    )


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        return int(raw)
    except ValueError:
        return default


def _env_float(name: str, default: float) -> float:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        return float(raw)
    except ValueError:
        return default


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
