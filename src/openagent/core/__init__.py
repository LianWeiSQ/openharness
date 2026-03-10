"""
core 包：

注意：这里不要在 import 时主动加载大量子模块，否则容易引发循环依赖。
建议业务侧按需从具体模块导入，例如：
- from openagent.core.loop.processor import AgentLoop
- from openagent.core.session.session import Session
"""

from __future__ import annotations

from typing import Any

__all__ = ["AgentLoop", "Session", "UniversalAgent"]


def __getattr__(name: str) -> Any:
    # PEP 562：延迟导入，避免循环依赖
    if name == "AgentLoop":
        from .loop.processor import AgentLoop as _AgentLoop

        return _AgentLoop
    if name == "Session":
        from .session.session import Session as _Session

        return _Session
    if name == "UniversalAgent":
        from .agent.universal import UniversalAgent as _UniversalAgent

        return _UniversalAgent
    raise AttributeError(name)
