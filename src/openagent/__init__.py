"""
OpenAgent core runtime (Python).

注意：为了避免循环依赖，这里使用“延迟导入”导出常用符号。
推荐业务侧也尽量从具体模块导入（更清晰、更稳定）。
"""

from __future__ import annotations

from typing import Any

__all__ = [
    "AgentLoop",
    "ExploreAgent",
    "PlanAgent",
    "Session",
    "UniversalAgent",
]


def __getattr__(name: str) -> Any:
    if name == "AgentLoop":
        from .core.loop.processor import AgentLoop as _AgentLoop

        return _AgentLoop
    if name == "Session":
        from .core.session.session import Session as _Session

        return _Session
    if name == "UniversalAgent":
        from .core.agent.universal import UniversalAgent as _UniversalAgent

        return _UniversalAgent
    if name == "PlanAgent":
        from .core.agent.plan import PlanAgent as _PlanAgent

        return _PlanAgent
    if name == "ExploreAgent":
        from .core.agent.explore import ExploreAgent as _ExploreAgent

        return _ExploreAgent
    raise AttributeError(name)
