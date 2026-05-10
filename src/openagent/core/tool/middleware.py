from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Any

from ..observability import ObservationRecorder, input_preview, output_stats
from ..permission.manager import PermissionDeniedError
from ..permission.rule import PermissionAction
from ..types import ToolCall, ToolResult

Next = Callable[[ToolCall], Awaitable[ToolResult]]
Middleware = Callable[[ToolCall, Next, dict[str, Any]], Awaitable[ToolResult]]


def permission_middleware(permission_manager) -> Middleware:
    async def _mw(call: ToolCall, nxt: Next, ctx: dict[str, Any]) -> ToolResult:
        action = await permission_manager.check({"name": call.name, "input": call.input, "call_id": call.call_id})
        if action == PermissionAction.DENY:
            recorder = ctx.get("observation_recorder")
            if isinstance(recorder, ObservationRecorder):
                recorder.event(
                    "permission.denied",
                    kind="permission",
                    status="error",
                    attributes={
                        "tool_name": call.name,
                        "call_id": call.call_id,
                        "error_kind": "permission_denied",
                    },
                )
            raise PermissionDeniedError(f"Permission denied for tool: {call.name}")
        return await nxt(call)

    return _mw


def observability_middleware() -> Middleware:
    async def _mw(call: ToolCall, nxt: Next, ctx: dict[str, Any]) -> ToolResult:
        recorder = ctx.get("observation_recorder")
        if not isinstance(recorder, ObservationRecorder):
            return await nxt(call)

        preview = input_preview(call.input, max_chars=recorder.config.input_preview_chars)
        with recorder.span(
            "tool.call",
            kind="tool",
            attributes={
                "tool_name": call.name,
                "call_id": call.call_id,
                "input_preview": preview,
                "execution_mode": ctx.get("execution_mode"),
            },
        ) as span:
            try:
                result = await nxt(call)
            except Exception as error:
                span.record_error(error, error_kind=type(error).__name__)
                raise

            metadata = result.metadata or {}
            span.set_attributes(
                {
                    "tool_name": call.name,
                    "call_id": result.call_id,
                    "error": bool(result.error),
                    "error_kind": metadata.get("error_kind"),
                    "title": metadata.get("title"),
                    "truncated": bool(metadata.get("truncated")),
                    "output_truncated": bool(metadata.get("output_truncated")),
                    "output_path": metadata.get("output_path"),
                    **output_stats(result.output),
                }
            )
            if result.error:
                span.status = "error"
                span.error = {
                    "type": str(metadata.get("error_kind") or "tool_error"),
                    "message": str(result.error),
                }
            return result

    return _mw


def logging_middleware(logger: list[dict[str, Any]]) -> Middleware:
    async def _mw(call: ToolCall, nxt: Next, ctx: dict[str, Any]) -> ToolResult:
        logger.append({"event": "tool.call", "name": call.name, "call_id": call.call_id})
        result = await nxt(call)
        logger.append({"event": "tool.result", "name": call.name, "call_id": call.call_id, "error": result.error})
        return result

    return _mw
