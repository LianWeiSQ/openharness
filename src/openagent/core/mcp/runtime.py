from __future__ import annotations

import asyncio
import hashlib
import json
import re
import threading
import time
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from datetime import timedelta
from fnmatch import fnmatchcase
from typing import Any, Literal

import httpx
from mcp import ClientSession
from mcp.client.sse import sse_client
from mcp.client.streamable_http import streamable_http_client
from mcp.types import EmbeddedResource, ImageContent, TextContent

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


@dataclass(slots=True)
class _ServerState:
    config: RemoteMcpServerConfig
    status: str = "idle"
    selected_transport: Literal["http", "sse"] | None = None
    last_error: str | None = None
    last_refreshed_at: float | None = None
    tools_by_dynamic_name: dict[str, RemoteMcpToolDescriptor] = field(default_factory=dict)


class RemoteMcpManager:
    def __init__(self, config: McpConfig) -> None:
        self.config = config
        self._lock = threading.RLock()
        self._servers: dict[str, _ServerState] = {
            server.name: _ServerState(config=server, status="disabled" if not server.enabled else "idle")
            for server in config.servers
        }

    @property
    def enabled(self) -> bool:
        return self.config.enabled

    def snapshot(self) -> RemoteMcpSnapshot:
        with self._lock:
            servers = [
                RemoteMcpServerSnapshot(
                    name=state.config.name,
                    url=state.config.url,
                    enabled=state.config.enabled,
                    configured_transport=state.config.transport,
                    selected_transport=state.selected_transport,
                    status=state.status,
                    tool_count=len(state.tools_by_dynamic_name),
                    last_error=state.last_error,
                    last_refreshed_at=state.last_refreshed_at,
                    tools=[
                        {
                            "name": descriptor.dynamic_name,
                            "original_name": descriptor.original_name,
                            "title": descriptor.title,
                            "description": descriptor.description,
                        }
                        for descriptor in sorted(
                            state.tools_by_dynamic_name.values(),
                            key=lambda item: item.dynamic_name,
                        )
                    ],
                )
                for state in self._servers.values()
            ]
        return RemoteMcpSnapshot(
            configured=bool(self._servers),
            enabled=self.enabled,
            server_count=len(servers),
            servers=servers,
        )

    def list_tool_descriptors(self) -> list[RemoteMcpToolDescriptor]:
        with self._lock:
            tools: list[RemoteMcpToolDescriptor] = []
            for state in self._servers.values():
                tools.extend(state.tools_by_dynamic_name.values())
        return sorted(tools, key=lambda item: item.dynamic_name)

    def refresh_all_sync(self) -> None:
        if not self._servers:
            return
        asyncio.run(self.refresh_all())

    def refresh_if_stale_sync(self) -> None:
        if not self._servers:
            return
        asyncio.run(self.refresh_if_stale())

    async def refresh_if_stale(self) -> None:
        now = time.time()
        with self._lock:
            needs_refresh = any(
                state.config.enabled
                and (
                    state.last_refreshed_at is None
                    or (now - state.last_refreshed_at) >= self.config.refresh_ttl_s
                )
                for state in self._servers.values()
            )
        if needs_refresh:
            await self.refresh_all()

    async def refresh_all(self) -> None:
        for server in self.config.servers:
            await self._refresh_server(server.name)

    async def call_tool(self, dynamic_name: str, arguments: dict[str, Any] | None) -> RemoteMcpToolCallResult:
        descriptor = self._find_descriptor(dynamic_name)
        if descriptor is None:
            await self.refresh_all()
            descriptor = self._find_descriptor(dynamic_name)
        if descriptor is None:
            return RemoteMcpToolCallResult(
                output="",
                error=f"Remote MCP tool '{dynamic_name}' is not available.",
                metadata={"tool": dynamic_name, "backend": "mcp"},
            )

        server = self._servers[descriptor.server_name].config
        try:
            transport, result = await self._call_tool_with_fallback(server, descriptor.original_name, arguments or {})
        except Exception as exc:  # noqa: BLE001
            with self._lock:
                state = self._servers[descriptor.server_name]
                state.status = "error"
                state.last_error = str(exc)
            return RemoteMcpToolCallResult(
                output="",
                error=str(exc),
                metadata={
                    "backend": "mcp",
                    "mcp_server": descriptor.server_name,
                    "mcp_tool": descriptor.original_name,
                    "transport": self._servers[descriptor.server_name].selected_transport,
                },
            )

        metadata = {
            "backend": "mcp",
            "mcp_server": descriptor.server_name,
            "mcp_tool": descriptor.original_name,
            "transport": transport,
            "is_error": bool(getattr(result, "isError", False)),
        }

        output = _render_tool_result_output(result)
        error = None
        if bool(getattr(result, "isError", False)):
            error = output or "Remote MCP tool returned an error."
            output = ""
        elif not output:
            output = "(Remote MCP tool completed with no textual output.)"

        structured = getattr(result, "structuredContent", None)
        if structured is not None:
            metadata["structured_content"] = _json_safe(structured)

        return RemoteMcpToolCallResult(output=output, error=error, metadata=metadata)

    async def _refresh_server(self, server_name: str) -> None:
        state = self._servers[server_name]
        config = state.config
        if not config.enabled:
            with self._lock:
                state.status = "disabled"
                state.selected_transport = None
                state.last_error = None
                state.last_refreshed_at = time.time()
                state.tools_by_dynamic_name = {}
            return

        with self._lock:
            state.status = "refreshing"
            state.last_error = None

        try:
            transport, tools = await self._list_tools_with_fallback(config)
            descriptors = _build_tool_descriptors(config, tools)
            with self._lock:
                state.status = "ready"
                state.selected_transport = transport
                state.last_error = None
                state.last_refreshed_at = time.time()
                state.tools_by_dynamic_name = {item.dynamic_name: item for item in descriptors}
        except Exception as exc:  # noqa: BLE001
            with self._lock:
                state.status = "error"
                state.selected_transport = None
                state.last_error = str(exc)
                state.last_refreshed_at = time.time()
                state.tools_by_dynamic_name = {}

    def _find_descriptor(self, dynamic_name: str) -> RemoteMcpToolDescriptor | None:
        with self._lock:
            for state in self._servers.values():
                descriptor = state.tools_by_dynamic_name.get(dynamic_name)
                if descriptor is not None:
                    return descriptor
        return None

    async def _list_tools_with_fallback(
        self, server: RemoteMcpServerConfig
    ) -> tuple[Literal["http", "sse"], list[Any]]:
        errors: list[str] = []
        for transport in _transport_candidates(server.transport):
            try:
                tools = await self._list_tools(server, transport)
                return transport, tools
            except Exception as exc:  # noqa: BLE001
                errors.append(f"{transport}: {exc}")
                if server.transport != "auto":
                    break
        raise RuntimeError(f"Failed to list tools from MCP server '{server.name}' ({'; '.join(errors)})")

    async def _call_tool_with_fallback(
        self,
        server: RemoteMcpServerConfig,
        tool_name: str,
        arguments: dict[str, Any],
    ) -> tuple[Literal["http", "sse"], Any]:
        preferred: list[Literal["http", "sse"]]
        with self._lock:
            selected = self._servers[server.name].selected_transport
        if server.transport == "auto" and selected in {"http", "sse"}:
            other = "sse" if selected == "http" else "http"
            preferred = [selected, other]
        else:
            preferred = list(_transport_candidates(server.transport))

        errors: list[str] = []
        for transport in preferred:
            try:
                result = await self._call_tool(server, transport, tool_name, arguments)
                with self._lock:
                    state = self._servers[server.name]
                    state.selected_transport = transport
                    state.status = "ready"
                    state.last_error = None
                return transport, result
            except Exception as exc:  # noqa: BLE001
                errors.append(f"{transport}: {exc}")
                if server.transport != "auto":
                    break
        raise RuntimeError(
            f"Failed to call MCP tool '{tool_name}' on server '{server.name}' ({'; '.join(errors)})"
        )

    async def _list_tools(self, server: RemoteMcpServerConfig, transport: Literal["http", "sse"]) -> list[Any]:
        async with self._open_session(server, transport) as session:
            cursor: str | None = None
            tools: list[Any] = []
            while True:
                async with asyncio.timeout(_timeout_seconds(server.timeout_ms)):
                    result = await session.list_tools(cursor=cursor)
                tools.extend(list(getattr(result, "tools", []) or []))
                cursor = getattr(result, "nextCursor", None)
                if not cursor:
                    break
            return tools

    async def _call_tool(
        self,
        server: RemoteMcpServerConfig,
        transport: Literal["http", "sse"],
        tool_name: str,
        arguments: dict[str, Any],
    ) -> Any:
        async with self._open_session(server, transport) as session:
            async with asyncio.timeout(_timeout_seconds(server.timeout_ms)):
                return await session.call_tool(
                    tool_name,
                    arguments=arguments,
                    read_timeout_seconds=timedelta(seconds=_timeout_seconds(server.timeout_ms)),
                )

    @asynccontextmanager
    async def _open_session(
        self,
        server: RemoteMcpServerConfig,
        transport: Literal["http", "sse"],
    ):
        timeout_seconds = _timeout_seconds(server.timeout_ms)
        client = httpx.AsyncClient(headers=server.headers, timeout=httpx.Timeout(timeout_seconds))
        try:
            if transport == "http":
                async with streamable_http_client(server.url, http_client=client) as streams:
                    read_stream, write_stream, _session_id = streams
                    async with ClientSession(
                        read_stream,
                        write_stream,
                        read_timeout_seconds=timedelta(seconds=timeout_seconds),
                    ) as session:
                        await session.initialize()
                        yield session
                return

            async with sse_client(
                server.url,
                headers=server.headers,
                timeout=timeout_seconds,
                sse_read_timeout=max(timeout_seconds, 60.0),
            ) as streams:
                read_stream, write_stream = streams
                async with ClientSession(
                    read_stream,
                    write_stream,
                    read_timeout_seconds=timedelta(seconds=timeout_seconds),
                ) as session:
                    await session.initialize()
                    yield session
        finally:
            await client.aclose()


def _build_tool_descriptors(
    server: RemoteMcpServerConfig,
    tools: list[Any],
) -> list[RemoteMcpToolDescriptor]:
    descriptors: list[RemoteMcpToolDescriptor] = []
    seen: set[str] = set()

    for tool in tools:
        original_name = str(getattr(tool, "name", "") or "").strip()
        if not original_name or not _tool_allowed(original_name, server.tools):
            continue

        dynamic_name = _dynamic_tool_name(server.name, original_name)
        if dynamic_name in seen:
            suffix = hashlib.sha1(f"{server.name}:{original_name}".encode("utf-8")).hexdigest()[:6]
            dynamic_name = f"{dynamic_name}_{suffix}"
        seen.add(dynamic_name)

        title = str(getattr(tool, "title", "") or original_name)
        description = _tool_description(server.name, tool)
        schema = _normalize_input_schema(getattr(tool, "inputSchema", None))
        annotations = _json_safe(getattr(tool, "annotations", None)) or {}
        raw_metadata = {
            "title": title,
            "description": str(getattr(tool, "description", "") or ""),
            "annotations": annotations,
            "execution": _json_safe(getattr(tool, "execution", None)),
        }
        descriptors.append(
            RemoteMcpToolDescriptor(
                server_name=server.name,
                original_name=original_name,
                dynamic_name=dynamic_name,
                title=title,
                description=description,
                input_schema=schema,
                annotations=annotations if isinstance(annotations, dict) else {},
                raw_metadata=raw_metadata,
            )
        )
    return descriptors


def _tool_description(server_name: str, tool: Any) -> str:
    description = str(getattr(tool, "description", "") or "").strip()
    base = f"Remote MCP tool from server '{server_name}'. Original MCP tool name: '{getattr(tool, 'name', '')}'."
    return f"{base}\n\n{description}" if description else base


def _normalize_input_schema(raw: Any) -> dict[str, Any]:
    schema = _json_safe(raw)
    if not isinstance(schema, dict):
        return {"type": "object", "properties": {}}
    if schema.get("type") != "object":
        schema = {"type": "object", "properties": {}, "x-mcp-original-schema": schema}
    schema.setdefault("type", "object")
    schema.setdefault("properties", {})
    return schema


def _tool_allowed(tool_name: str, filters: McpToolFilter) -> bool:
    if filters.allow and not any(fnmatchcase(tool_name, pattern) for pattern in filters.allow):
        return False
    if filters.deny and any(fnmatchcase(tool_name, pattern) for pattern in filters.deny):
        return False
    return True


def _transport_candidates(transport: McpTransport) -> tuple[Literal["http", "sse"], ...]:
    if transport == "http":
        return ("http",)
    if transport == "sse":
        return ("sse",)
    return ("http", "sse")


def _dynamic_tool_name(server_name: str, tool_name: str) -> str:
    return f"mcp_tool_{_sanitize_name(server_name)}_{_sanitize_name(tool_name)}"


def _sanitize_name(value: str) -> str:
    lowered = re.sub(r"[^a-zA-Z0-9]+", "_", value.strip().lower()).strip("_")
    return lowered or "tool"


def _timeout_seconds(timeout_ms: int) -> float:
    return max(float(timeout_ms) / 1000.0, 1.0)


def _render_tool_result_output(result: Any) -> str:
    parts: list[str] = []
    for item in list(getattr(result, "content", []) or []):
        if isinstance(item, TextContent):
            text = str(getattr(item, "text", "") or "").strip()
            if text:
                parts.append(text)
            continue
        if isinstance(item, ImageContent):
            mime_type = str(getattr(item, "mimeType", "") or "image")
            parts.append(f"[image content omitted: {mime_type}]")
            continue
        if isinstance(item, EmbeddedResource):
            resource = getattr(item, "resource", None)
            parts.append(_render_embedded_resource(resource))
            continue

        dumped = _json_safe(item)
        if dumped:
            parts.append(json.dumps(dumped, ensure_ascii=False, indent=2))

    if parts:
        return "\n\n".join(part for part in parts if part)

    structured = getattr(result, "structuredContent", None)
    if structured is not None:
        return json.dumps(_json_safe(structured), ensure_ascii=False, indent=2)
    return ""


def _render_embedded_resource(resource: Any) -> str:
    if resource is None:
        return "[embedded resource omitted]"
    payload = _json_safe(resource)
    if isinstance(payload, dict):
        uri = payload.get("uri") or payload.get("name") or payload.get("title")
        mime_type = payload.get("mimeType")
        label = f": {uri}" if uri else ""
        if mime_type:
            return f"[embedded resource omitted{label} ({mime_type})]"
        return f"[embedded resource omitted{label}]"
    return "[embedded resource omitted]"


def _json_safe(value: Any) -> Any:
    if value is None:
        return None
    if isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, dict):
        return {str(key): _json_safe(item) for key, item in value.items()}
    if isinstance(value, (list, tuple, set)):
        return [_json_safe(item) for item in value]
    if hasattr(value, "model_dump"):
        return _json_safe(value.model_dump(mode="json", by_alias=True, exclude_none=True))
    if hasattr(value, "__dict__"):
        return _json_safe(vars(value))
    return str(value)

