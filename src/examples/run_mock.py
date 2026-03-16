from __future__ import annotations

import asyncio
from pathlib import Path
import sys

# Allow running this file directly (without installing / PYTHONPATH).
def _find_repo_root() -> Path:
    here = Path(__file__).resolve()
    for p in here.parents:
        if (p / "Agent.md").exists() and (p / "openagent").exists():
            return p
    return here.parents[3]


REPO_ROOT = _find_repo_root()
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.session.session import Session
from openagent.core.types import AgentConfig


class ScriptedModel:
    def __init__(self) -> None:
        self.calls = 0

    async def stream(self, *, system, messages, tools, temperature=None, max_output_tokens=None, options=None):
        self.calls += 1
        if self.calls == 1:
            yield {
                "type": "tool-call",
                "call_id": "c1",
                "name": "write",
                "input": {"file_path": "hello.txt", "content": "hello from openagent"},
            }
            yield {"type": "finish", "finish_reason": "tool_call", "usage": {}}
            return
        yield {"type": "text-delta", "id": "t1", "text": "done"}
        yield {"type": "finish", "finish_reason": "stop", "usage": {}}


async def main() -> None:
    workdir = Path("openagent/examples/workdir")
    workdir.mkdir(parents=True, exist_ok=True)

    agent = UniversalAgent(
        config=AgentConfig(name="universal", permission="FULL", max_steps=5),
        model=ScriptedModel(),
        system_prompt="",
    )
    loop = AgentLoop(agent=agent, session=Session(directory=workdir), permission_manager=PermissionManager())
    async for event in loop.run("create a file"):
        print(event)


if __name__ == "__main__":
    asyncio.run(main())
