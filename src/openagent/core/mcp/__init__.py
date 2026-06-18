from .config import load_mcp_config, load_mcp_config_from_sources
from .runtime import RemoteMcpManager
from .types import (
    McpConfig,
    McpToolFilter,
    McpTransport,
    RemoteMcpServerConfig,
    RemoteMcpServerSnapshot,
    RemoteMcpSnapshot,
    RemoteMcpToolCallResult,
    RemoteMcpToolDescriptor,
)

__all__ = [
    "McpConfig",
    "McpToolFilter",
    "McpTransport",
    "RemoteMcpServerConfig",
    "RemoteMcpServerSnapshot",
    "RemoteMcpSnapshot",
    "RemoteMcpToolCallResult",
    "RemoteMcpToolDescriptor",
    "RemoteMcpManager",
    "load_mcp_config",
    "load_mcp_config_from_sources",
    "register_mcp_tools",
]


def __getattr__(name: str):
    if name == "register_mcp_tools":
        from .bridge import register_mcp_tools

        return register_mcp_tools
    raise AttributeError(name)
