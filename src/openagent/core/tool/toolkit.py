from __future__ import annotations

"""Tool registration, exposure, and execution helpers."""

import asyncio
import warnings
from collections.abc import Awaitable, Callable
from dataclasses import fields, is_dataclass
from pathlib import Path
from typing import Any

from ..id import new_id
from ..types import ToolCall, ToolResult, ToolSchema
from .definition import ToolContext, ToolDefinition, ToolOutput
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

    # ---------------------------------------------------------------------
    # Loading / Registration
    # ---------------------------------------------------------------------

    def load_builtin(self) -> None:
        """Register built-in tools into the registry."""

        from .builtin import register_builtin_tools

        register_builtin_tools(self.registry)

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
        )
        self.registry.register(tool)

    def register_mcp(self, client: object, group: str = "mcp") -> None:
        """Reserved compatibility API for future MCP integration."""

        raise NotImplementedError(
            "MCP integration is not implemented yet; use register(registry) or load_plugins() for now."
        )

    def register_middleware(self, middleware: Middleware) -> None:
        self._middleware.append(middleware)

    # ---------------------------------------------------------------------
    # Tool exposure (to LLM)
    # ---------------------------------------------------------------------

    def get_all_tools(self) -> list[ToolSchema]:
        tools: list[ToolSchema] = []
        for t in self.registry.all():
            tools.append(
                ToolSchema(
                    name=t.id,
                    description=t.description,
                    schema=t.parameters_schema(),
                    group=t.group,
                    dangerous=t.dangerous,
                )
            )
        return tools

    def get_tools_by_group(self, groups: list[str]) -> list[ToolSchema]:
        allowed = set(groups)
        return [t for t in self.get_all_tools() if t.group in allowed]

    # ---------------------------------------------------------------------
    # Execution
    # ---------------------------------------------------------------------

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

        call = ToolCall(name=name, input=input, call_id=call_id or new_id("toolcall"))
        ctx = context or {}

        async def _invoke(c: ToolCall) -> ToolResult:
            session_root_value = ctx.get("session_root") or ctx.get("cwd")
            session_root = Path(str(session_root_value or Path.cwd())).resolve()
            tool_ctx = ToolContext(
                session_id=str(ctx.get("session_id") or ""),
                session_root=session_root,
                call_id=c.call_id,
                extra={k: v for k, v in ctx.items() if k not in ("session_root", "cwd", "session_id")},
            )
            try:
                args_obj = _coerce_params(tool.parameters, c.input)
            except Exception as e:  # noqa: BLE001
                return ToolResult(call_id=c.call_id, output="", error=str(e), metadata={"tool": tool.id})

            try:
                out = tool.execute(args_obj, tool_ctx)
                if asyncio.iscoroutine(out) or isinstance(out, Awaitable):
                    tool_out: ToolOutput = await out  # type: ignore[assignment]
                else:
                    tool_out = out  # type: ignore[assignment]
            except Exception as e:  # noqa: BLE001
                return ToolResult(call_id=c.call_id, output="", error=str(e), metadata={"tool": tool.id})

            raw_output = tool_out.output or ""
            truncated_output = Truncate.output(raw_output)
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
                output_path = _write_truncated_output(session_root, c.call_id, raw_output)
                metadata["output_path"] = str(output_path)

            return ToolResult(
                call_id=c.call_id,
                output=output_text,
                error=tool_out.error,
                metadata=metadata,
            )

        handler = _invoke
        for mw in reversed(self._middleware):
            nxt = handler

            async def handler(c: ToolCall, mw=mw, nxt=nxt):  # type: ignore[misc]
                return await mw(c, nxt, ctx)

        return await handler(call)


def _coerce_params(parameters_type: type, payload: dict[str, Any]) -> Any:
    """Coerce a model-produced dict payload into the configured parameter type."""

    if not is_dataclass(parameters_type):
        return payload

    allowed = {f.name for f in fields(parameters_type)}
    filtered = {k: v for k, v in (payload or {}).items() if k in allowed}
    return parameters_type(**filtered)


def _legacy_context_from_tool_context(tool_ctx: ToolContext) -> dict[str, Any]:
    """Convert the structured tool context back into the legacy dict shape."""

    legacy_ctx = dict(tool_ctx.extra)
    legacy_ctx["session_id"] = tool_ctx.session_id
    legacy_ctx["session_root"] = str(tool_ctx.session_root)
    legacy_ctx.setdefault("cwd", str(tool_ctx.session_root))
    legacy_ctx["call_id"] = tool_ctx.call_id
    return legacy_ctx


def _write_truncated_output(session_root: Path, call_id: str, content: str) -> Path:
    out_dir = session_root / ".openagent" / "tool_output"
    out_dir.mkdir(parents=True, exist_ok=True)
    p = out_dir / f"{call_id}.txt"
    p.write_text(content, encoding="utf-8")
    return p


__all__ = ["ToolkitAdapter", "ToolNotFoundError"]
