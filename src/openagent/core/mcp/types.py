from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Literal

McpTransport = Literal["auto", "http", "sse"]


@dataclass(frozen=True, slots=True)
class McpToolFilter:
    allow: tuple[str, ...] = ("*",)
    deny: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class RemoteMcpServerConfig:
    name: str
    url: str
    transport: McpTransport = "auto"
    enabled: bool = True
    headers: dict[str, str] = field(default_factory=dict)
    timeout_ms: int = 30000
    tools: McpToolFilter = field(default_factory=McpToolFilter)


@dataclass(frozen=True, slots=True)
class McpConfig:
    servers: tuple[RemoteMcpServerConfig, ...] = ()
    refresh_ttl_s: float = 30.0

    @property
    def enabled(self) -> bool:
        return any(server.enabled for server in self.servers)


@dataclass(frozen=True, slots=True)
class RemoteMcpToolDescriptor:
    server_name: str
    original_name: str
    dynamic_name: str
    title: str
    description: str
    input_schema: dict[str, Any]
    annotations: dict[str, Any] = field(default_factory=dict)
    raw_metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class RemoteMcpToolCallResult:
    output: str
    error: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class RemoteMcpServerSnapshot:
    name: str
    url: str
    enabled: bool
    configured_transport: McpTransport
    selected_transport: Literal["http", "sse"] | None
    status: str
    tool_count: int
    last_error: str | None = None
    last_refreshed_at: float | None = None
    tools: list[dict[str, Any]] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "url": self.url,
            "enabled": self.enabled,
            "configured_transport": self.configured_transport,
            "selected_transport": self.selected_transport,
            "status": self.status,
            "tool_count": self.tool_count,
            "last_error": self.last_error,
            "last_refreshed_at": self.last_refreshed_at,
            "tools": list(self.tools),
        }


@dataclass(frozen=True, slots=True)
class RemoteMcpSnapshot:
    configured: bool
    enabled: bool
    server_count: int
    servers: list[RemoteMcpServerSnapshot] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "configured": self.configured,
            "enabled": self.enabled,
            "server_count": self.server_count,
            "servers": [server.to_dict() for server in self.servers],
        }

