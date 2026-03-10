from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Any

from ..permission.manager import PermissionDeniedError
from ..permission.rule import PermissionAction
from ..types import ToolCall, ToolResult

Next = Callable[[ToolCall], Awaitable[ToolResult]]
Middleware = Callable[[ToolCall, Next, dict[str, Any]], Awaitable[ToolResult]]


def permission_middleware(permission_manager) -> Middleware:
    async def _mw(call: ToolCall, nxt: Next, ctx: dict[str, Any]) -> ToolResult:
        action = await permission_manager.check({"name": call.name, "input": call.input, "call_id": call.call_id})
        if action == PermissionAction.DENY:
            raise PermissionDeniedError(f"Permission denied for tool: {call.name}")
        return await nxt(call)

    return _mw


def logging_middleware(logger: list[dict[str, Any]]) -> Middleware:
    async def _mw(call: ToolCall, nxt: Next, ctx: dict[str, Any]) -> ToolResult:
        logger.append({"event": "tool.call", "name": call.name, "call_id": call.call_id})
        result = await nxt(call)
        logger.append({"event": "tool.result", "name": call.name, "call_id": call.call_id, "error": result.error})
        return result

    return _mw

