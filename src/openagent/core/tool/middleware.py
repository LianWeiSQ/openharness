from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Any

from ..observability import ObservationRecorder, input_preview, output_stats
from ..permission.manager import PermissionDeniedError
from ..permission.rule import PermissionAction
from ..runtime_logging import RuntimeLogger
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
            runtime_logger = ctx.get("runtime_logger")
            if isinstance(runtime_logger, RuntimeLogger):
                runtime_logger.warning(
                    "Tool permission denied",
                    category="permission",
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
        tool_definition = _tool_definition(ctx)
        with recorder.span(
            "tool.call",
            kind="tool",
            attributes={
                "tool_name": call.name,
                "call_id": call.call_id,
                "input_preview": preview,
                "execution_mode": ctx.get("execution_mode"),
                "tool_group": tool_definition.get("group"),
                "dangerous": tool_definition.get("dangerous"),
                "execution_scope": tool_definition.get("execution_scope"),
                "tool_source": _tool_source(call.name, tool_definition=tool_definition),
            },
        ) as span:
            try:
                result = await nxt(call)
            except Exception as error:
                span.record_error(error, error_kind=type(error).__name__)
                raise

            metadata = result.metadata or {}
            source = _tool_source(call.name, tool_definition=tool_definition, metadata=metadata)
            span.set_attributes(
                {
                    "tool_name": call.name,
                    "call_id": result.call_id,
                    "tool_source": source,
                    "tool_group": tool_definition.get("group"),
                    "backend": metadata.get("backend"),
                    "mcp_server": metadata.get("mcp_server"),
                    "mcp_original_tool_name": metadata.get("mcp_original_tool_name"),
                    "mcp_tool_name": metadata.get("mcp_tool_name"),
                    "skill_name": metadata.get("skill_name"),
                    "skill_location": metadata.get("skill_location"),
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


def _tool_definition(ctx: dict[str, Any]) -> dict[str, Any]:
    value = ctx.get("tool_definition")
    return dict(value) if isinstance(value, dict) else {}


def _tool_source(
    tool_name: str,
    *,
    tool_definition: dict[str, Any],
    metadata: dict[str, Any] | None = None,
) -> str:
    metadata = metadata or {}
    backend = str(metadata.get("backend") or "").strip()
    if backend == "mcp":
        return "mcp"
    group = str(tool_definition.get("group") or "").strip()
    if group == "skill" or tool_name == "skill" or metadata.get("skill_name"):
        return "skill"
    if group == "mcp" or metadata.get("mcp_server"):
        return "mcp"
    execution_mode = str(metadata.get("execution_mode") or "").strip()
    if execution_mode == "opensandbox":
        return "sandbox"
    return "local_tool"


def logging_middleware(logger: list[dict[str, Any]]) -> Middleware:
    async def _mw(call: ToolCall, nxt: Next, ctx: dict[str, Any]) -> ToolResult:
        runtime_logger = ctx.get("runtime_logger")
        logger.append({"event": "tool.call", "name": call.name, "call_id": call.call_id})
        if isinstance(runtime_logger, RuntimeLogger):
            runtime_logger.debug(
                "Tool call started",
                category="tool",
                attributes={
                    "tool_name": call.name,
                    "call_id": call.call_id,
                    "input_preview": input_preview(call.input, max_chars=runtime_logger.config.input_preview_chars),
                },
            )
        try:
            result = await nxt(call)
        except Exception as error:
            logger.append({"event": "tool.error", "name": call.name, "call_id": call.call_id, "error": str(error)})
            if isinstance(runtime_logger, RuntimeLogger):
                runtime_logger.error(
                    "Tool call raised an exception",
                    category="tool",
                    attributes={
                        "tool_name": call.name,
                        "call_id": call.call_id,
                        "error_kind": type(error).__name__,
                        "message": str(error),
                    },
                )
            raise
        logger.append({"event": "tool.result", "name": call.name, "call_id": call.call_id, "error": result.error})
        if isinstance(runtime_logger, RuntimeLogger):
            level = "ERROR" if result.error else "DEBUG"
            runtime_logger.log(
                level,
                "Tool call finished" if not result.error else "Tool call failed",
                category="tool",
                attributes={
                    "tool_name": call.name,
                    "call_id": result.call_id,
                    "error": bool(result.error),
                    "error_kind": (result.metadata or {}).get("error_kind"),
                    "output_truncated": bool((result.metadata or {}).get("output_truncated")),
                    "output_path": (result.metadata or {}).get("output_path"),
                    **output_stats(result.output),
                },
            )
        return result

    return _mw
