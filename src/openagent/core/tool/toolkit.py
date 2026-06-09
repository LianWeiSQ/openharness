from __future__ import annotations

"""Tool registration, exposure, and execution helpers."""

import asyncio
import warnings
from collections.abc import Awaitable, Callable
from dataclasses import fields, is_dataclass
from enum import Enum
from pathlib import Path
from types import UnionType
from typing import Any, get_args, get_origin, get_type_hints

from ..context_budget import ContextBudgetConfigError, load_context_budget_options
from ..id import new_id
from ..types import ToolCall, ToolResult, ToolSchema
from .definition import ToolContext, ToolDefinition, ToolExecutionScope, ToolOutput
from .middleware import Middleware
from .registry import ToolRegistry
from .truncation import Truncate

LegacyToolFunc = Callable[[dict[str, Any], dict[str, Any]], Awaitable[Any] | Any]


class ToolNotFoundError(KeyError):
    pass


class ToolkitAdapter:
    def __init__(self, *, registry: ToolRegistry | None = None) -> None:
        self.registry = registry or ToolRegistry()
        self._middleware: list[Middleware] = []
        self._builtin_loaded = False

    def load_builtin(self) -> None:
        """Register built-in tools into the registry."""

        if self._builtin_loaded:
            return
        from .builtin import register_builtin_tools

        register_builtin_tools(self.registry)
        self._builtin_loaded = True

    def load_plugins(self, *, tool_paths: list[str], base_dir: Path) -> None:
        self.registry.load_plugins(tool_paths=tool_paths, base_dir=base_dir)

    def register_tool(
        self,
        name: str,
        func: LegacyToolFunc,
        description: str = "",
        schema: dict[str, Any] | None = None,
        group: str = "default",
        dangerous: bool = False,
        execution_scope: ToolExecutionScope = "host_only",
    ) -> None:
        """Deprecated compatibility shim for legacy function-style tools."""

        warnings.warn(
            "ToolkitAdapter.register_tool() is deprecated; prefer register(registry) + ToolRegistry.define_tool().",
            DeprecationWarning,
            stacklevel=2,
        )

        async def _execute(args: dict[str, Any], tool_ctx: ToolContext) -> ToolOutput:
            legacy_ctx = _legacy_context_from_tool_context(tool_ctx)
            result = func(args, legacy_ctx)
            if asyncio.iscoroutine(result) or isinstance(result, Awaitable):
                result = await result
            if isinstance(result, ToolOutput):
                return result
            return ToolOutput(title=name, output="" if result is None else str(result), metadata={})

        tool = ToolDefinition(
            id=name,
            description=description,
            parameters=dict,
            execute=_execute,
            dangerous=dangerous,
            group=group,
            schema_override=schema or {"type": "object", "properties": {}},
            execution_scope=execution_scope,
        )
        self.registry.register(tool)

    def register_mcp(self, client: object, group: str = "mcp") -> None:
        """Register tool-only MCP tools exposed by a remote MCP manager."""

        from ..mcp.bridge import register_mcp_tools

        register_mcp_tools(self.registry, client, group=group)

    def register_middleware(self, middleware: Middleware) -> None:
        self._middleware.append(middleware)

    def get_all_tools(self, *, execution_mode: str = "local") -> list[ToolSchema]:
        tools: list[ToolSchema] = []
        for tool in self.registry.all():
            if not _tool_available(tool, execution_mode):
                continue
            tools.append(
                ToolSchema(
                    name=tool.id,
                    description=tool.description,
                    schema=tool.parameters_schema(),
                    group=tool.group,
                    dangerous=tool.dangerous,
                )
            )
        return tools

    def get_tools_by_group(self, groups: list[str], *, execution_mode: str = "local") -> list[ToolSchema]:
        allowed = set(groups)
        return [tool for tool in self.get_all_tools(execution_mode=execution_mode) if tool.group in allowed]

    async def execute(
        self,
        *,
        name: str,
        input: dict[str, Any],
        call_id: str | None = None,
        context: dict[str, Any] | None = None,
    ) -> ToolResult:
        tool = self.registry.get(name)
        if tool is None:
            raise ToolNotFoundError(name)

        ctx = dict(context or {})
        execution_mode = str(ctx.get("execution_mode") or "local")
        if not _tool_available(tool, execution_mode):
            return ToolResult(
                call_id=call_id or new_id("toolcall"),
                output="",
                error=f"Tool \"{name}\" is not available in {execution_mode} mode.",
                metadata={"tool": tool.id, "execution_mode": execution_mode, "error_kind": "execution_scope_unavailable"},
            )

        call = ToolCall(name=name, input=input, call_id=call_id or new_id("toolcall"))
        ctx = dict(context or {})
        ctx["tool_definition"] = {
            "id": tool.id,
            "group": tool.group,
            "dangerous": tool.dangerous,
            "execution_scope": tool.execution_scope,
        }

        async def _invoke(tool_call: ToolCall) -> ToolResult:
            session_root_value = ctx.get("session_root") or ctx.get("cwd")
            session_root = Path(str(session_root_value or Path.cwd())).resolve()
            tool_ctx = ToolContext(
                session_id=str(ctx.get("session_id") or ""),
                session_root=session_root,
                call_id=tool_call.call_id,
                extra={k: v for k, v in ctx.items() if k not in ("session_root", "cwd", "session_id", "execution_mode", "workspace_root", "workspace_runtime", "execution_metadata")},
                execution_mode=str(ctx.get("execution_mode") or "local"),
                workspace_root=str(ctx.get("workspace_root")) if ctx.get("workspace_root") is not None else None,
                workspace_runtime=ctx.get("workspace_runtime"),
                execution_metadata=dict(ctx.get("execution_metadata") or {}),
            )
            try:
                args_obj = _coerce_params(tool.parameters, tool_call.input)
            except Exception as error:  # noqa: BLE001
                return ToolResult(call_id=tool_call.call_id, output="", error=str(error), metadata={"tool": tool.id})

            try:
                out = tool.execute(args_obj, tool_ctx)
                if asyncio.iscoroutine(out) or isinstance(out, Awaitable):
                    tool_out: ToolOutput = await out  # type: ignore[assignment]
                else:
                    tool_out = out  # type: ignore[assignment]
            except Exception as error:  # noqa: BLE001
                return ToolResult(call_id=tool_call.call_id, output="", error=str(error), metadata={"tool": tool.id})

            raw_output = tool_out.output or ""
            truncated_output = Truncate.output(raw_output, options=_tool_truncation_options(ctx))
            tool_semantic_truncated = bool(tool_out.truncated or (tool_out.metadata or {}).get("truncated"))
            output_truncated = truncated_output.truncated

            metadata = dict(tool_out.metadata or {})
            metadata.setdefault("tool", tool.id)
            metadata.setdefault("title", tool_out.title)
            metadata["truncated"] = tool_semantic_truncated or output_truncated
            metadata["output_truncated"] = output_truncated
            metadata["original_lines"] = truncated_output.original_lines
            metadata["original_bytes"] = truncated_output.original_bytes

            output_text = truncated_output.content
            if output_truncated:
                output_path = _write_truncated_output(session_root, tool_call.call_id, raw_output)
                metadata["output_path"] = str(output_path)

            return ToolResult(
                call_id=tool_call.call_id,
                output=output_text,
                error=tool_out.error,
                metadata=metadata,
            )

        handler = _invoke
        for middleware in reversed(self._middleware):
            next_handler = handler

            async def handler(tool_call: ToolCall, middleware=middleware, next_handler=next_handler):  # type: ignore[misc]
                return await middleware(tool_call, next_handler, ctx)

        return await handler(call)


def _tool_available(tool: ToolDefinition, execution_mode: str) -> bool:
    if execution_mode != "opensandbox":
        return True
    return tool.execution_scope in {"workspace", "agnostic"}


def _tool_truncation_options(context: dict[str, Any]) -> dict[str, int]:
    agent_options = context.get("agent_options")
    if not isinstance(agent_options, dict):
        return {}

    try:
        config = load_context_budget_options(agent_options, model=None)
    except ContextBudgetConfigError:
        return {}

    return {
        "max_lines": Truncate.DEFAULT_MAX_LINES,
        "max_bytes": int(config["tool_display_max_bytes"]),
    }


def _coerce_params(parameters_type: type, payload: dict[str, Any]) -> Any:
    """Coerce a model-produced payload into the configured parameter type."""

    if not is_dataclass(parameters_type):
        return payload
    return _coerce_value(parameters_type, payload or {})


def _coerce_value(tp: Any, value: Any) -> Any:
    if value is None:
        return None

    inner = _unwrap_optional(tp)
    origin = get_origin(inner)
    args = get_args(inner)

    if inner is Any or inner is object:
        return value

    if origin in (list, tuple, set):
        if not isinstance(value, list):
            raise TypeError(f"Expected list input for {inner!r}")
        item_tp = args[0] if args else Any
        coerced = [_coerce_value(item_tp, item) for item in value]
        if origin is tuple:
            return tuple(coerced)
        if origin is set:
            return set(coerced)
        return coerced

    if origin is dict:
        if not isinstance(value, dict):
            raise TypeError(f"Expected object input for {inner!r}")
        key_tp = args[0] if len(args) > 0 else Any
        value_tp = args[1] if len(args) > 1 else Any
        return {
            _coerce_value(key_tp, key): _coerce_value(value_tp, item)
            for key, item in value.items()
        }

    if is_dataclass(inner):
        if isinstance(value, inner):
            return value
        if not isinstance(value, dict):
            raise TypeError(f"Expected object input for dataclass {inner.__name__}")
        try:
            hints = get_type_hints(inner, include_extras=True)
        except Exception:  # noqa: BLE001
            hints = {}
        kwargs: dict[str, Any] = {}
        for field in fields(inner):
            if field.name in value:
                kwargs[field.name] = _coerce_value(hints.get(field.name, field.type), value[field.name])
        return inner(**kwargs)

    if isinstance(inner, type) and issubclass(inner, Enum):
        if isinstance(value, inner):
            return value
        return inner(value)

    return value


def _unwrap_optional(tp: Any) -> Any:
    origin = get_origin(tp)
    if origin not in (UnionType, getattr(__import__("typing"), "Union")):
        return tp
    args = [arg for arg in get_args(tp) if arg is not type(None)]  # noqa: E721
    return args[0] if args else Any


def _legacy_context_from_tool_context(tool_ctx: ToolContext) -> dict[str, Any]:
    """Convert the structured tool context back into the legacy dict shape."""

    legacy_ctx = dict(tool_ctx.extra)
    legacy_ctx["session_id"] = tool_ctx.session_id
    legacy_ctx["session_root"] = str(tool_ctx.session_root)
    legacy_ctx.setdefault("cwd", str(tool_ctx.session_root))
    legacy_ctx["call_id"] = tool_ctx.call_id
    legacy_ctx["execution_mode"] = tool_ctx.execution_mode
    if tool_ctx.workspace_root is not None:
        legacy_ctx["workspace_root"] = tool_ctx.workspace_root
    if tool_ctx.workspace_runtime is not None:
        legacy_ctx["workspace_runtime"] = tool_ctx.workspace_runtime
    if tool_ctx.execution_metadata:
        legacy_ctx["execution_metadata"] = dict(tool_ctx.execution_metadata)
    return legacy_ctx


def _write_truncated_output(session_root: Path, call_id: str, content: str) -> Path:
    out_dir = session_root / ".openagent" / "tool_output"
    out_dir.mkdir(parents=True, exist_ok=True)
    output_path = out_dir / f"{call_id}.txt"
    output_path.write_text(content, encoding="utf-8")
    return output_path


__all__ = ["ToolkitAdapter", "ToolNotFoundError"]
