"""
adapter 包：

同样避免在 import 时主动加载子模块（减少循环依赖风险）。
"""

from __future__ import annotations

from typing import Any

__all__ = ["AgentAdapter"]


def __getattr__(name: str) -> Any:
    if name == "AgentAdapter":
        from .agent_adapter import AgentAdapter as _AgentAdapter

        return _AgentAdapter
    raise AttributeError(name)
