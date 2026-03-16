from __future__ import annotations


class MCPClientBase:
    """
    Placeholder for MCP integration.

    The compatibility method `ToolkitAdapter.register_mcp(client)` still exists,
    but it intentionally raises `NotImplementedError` for now. New extensions
    should use `register(registry)` plus `ToolRegistry.define_tool()` until the
    MCP bridge is implemented.
    """

    pass
