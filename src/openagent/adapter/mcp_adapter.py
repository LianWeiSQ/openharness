from __future__ import annotations

from typing import Protocol

from openagent.core.mcp.types import RemoteMcpToolDescriptor


class MCPClientBase(Protocol):
    """Compatibility protocol for tool-only MCP bridges."""

    def list_tool_descriptors(self) -> list[RemoteMcpToolDescriptor]: ...

    async def call_tool(self, dynamic_name: str, arguments: dict[str, object] | None) -> object: ...
