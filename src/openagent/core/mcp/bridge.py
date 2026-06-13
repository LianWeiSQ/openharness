from __future__ import annotations

from typing import Protocol

from ..tool.definition import ToolContext, ToolDefinition, ToolExecutionSchema, ToolOutput
from ..tool.registry import ToolRegistry
from .runtime import RemoteMcpManager
from .types import RemoteMcpToolDescriptor


class McpToolBridge(Protocol):
    def list_tool_descriptors(self) -> list[RemoteMcpToolDescriptor]: ...

    async def call_tool(self, dynamic_name: str, arguments: dict[str, object] | None) -> object: ...


def register_mcp_tools(registry: ToolRegistry, client: RemoteMcpManager, *, group: str = "mcp") -> None:
    for descriptor in client.list_tool_descriptors():
        registry.register(_build_tool_definition(client, descriptor, group=group))




def _build_tool_definition(
    client: RemoteMcpManager,
    descriptor: RemoteMcpToolDescriptor,
    *,
    group: str,
) -> ToolDefinition:
    async def _execute(args: dict[str, object], _ctx: ToolContext) -> ToolOutput:
        result = await client.call_tool(descriptor.dynamic_name, args)
        output = str(getattr(result, "output", "") or "")
        error = getattr(result, "error", None)
        metadata = dict(getattr(result, "metadata", {}) or {})
        title = f"MCP {descriptor.server_name}/{descriptor.original_name}"
        metadata.setdefault("tool", descriptor.dynamic_name)
        metadata.setdefault("title", title)
        metadata.setdefault("backend", "mcp")
        metadata.setdefault("mcp_server", descriptor.server_name)
        metadata.setdefault("mcp_original_tool_name", descriptor.original_name)
        metadata.setdefault("mcp_transport", None)
        metadata.setdefault("mcp_tool_name", descriptor.dynamic_name)
        metadata.setdefault("mcp_non_text_blocks", [])
        return ToolOutput(
            title=title,
            output=output,
            metadata=metadata,
            error=str(error) if error else None,
        )

    return ToolDefinition(
        id=descriptor.dynamic_name,
        description=descriptor.description,
        parameters=dict,
        execute=_execute,
        dangerous=True,
        group=group,
        schema_override=descriptor.input_schema,
        execution_scope="agnostic",
        execution_schema=ToolExecutionSchema(
            external_io=True,
            concurrency="unknown",
            batch_group=group,
        ),
    )
