from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

from .types import McpConfig, McpToolFilter, RemoteMcpServerConfig

SUPPORTED_TRANSPORTS = {"auto", "http", "sse"}


def load_mcp_config_from_sources(
    *,
    cli_value: str | None = None,
    env: dict[str, str] | None = None,
) -> McpConfig | None:
    source = cli_value
    if source is None:
        env_map = env if env is not None else os.environ
        source = env_map.get("OPENAGENT_MCP_CONFIG")
    if source is None or not str(source).strip():
        return None
    return load_mcp_config(source)


def load_mcp_config(source: str | Path | dict[str, Any]) -> McpConfig:
    raw = _load_raw_config(source)
    if not isinstance(raw, dict):
        raise ValueError("MCP config must be a JSON object.")

    mcp_block = raw.get("mcp", raw)
    if not isinstance(mcp_block, dict):
        raise ValueError("MCP config must contain an object-valued 'mcp' field.")

    refresh_ttl_s = _parse_float(raw.get("refresh_ttl_s", 30.0), default=30.0, minimum=0.0)
    servers: list[RemoteMcpServerConfig] = []
    for server_name, server_raw in mcp_block.items():
        if not isinstance(server_name, str) or not server_name.strip():
            raise ValueError("MCP server names must be non-empty strings.")
        if not isinstance(server_raw, dict):
            raise ValueError(f"MCP server '{server_name}' must be configured with an object.")
        servers.append(_parse_server_config(server_name.strip(), server_raw))
    return McpConfig(servers=tuple(servers), refresh_ttl_s=refresh_ttl_s)


def _load_raw_config(source: str | Path | dict[str, Any]) -> dict[str, Any]:
    if isinstance(source, dict):
        return source

    path: Path | None = None
    if isinstance(source, Path):
        path = source
    else:
        text = str(source).strip()
        if not text:
            raise ValueError("MCP config source is empty.")
        candidate = Path(text).expanduser()
        if candidate.exists():
            path = candidate
        else:
            try:
                parsed = json.loads(text)
            except json.JSONDecodeError as exc:
                raise ValueError(
                    "MCP config must be a valid JSON string or a path to a JSON file."
                ) from exc
            if not isinstance(parsed, dict):
                raise ValueError("MCP config JSON must be an object.")
            return parsed

    assert path is not None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ValueError(f"MCP config file not found: {path}") from exc
    except json.JSONDecodeError as exc:
        raise ValueError(f"MCP config file is not valid JSON: {path}") from exc


def _parse_server_config(name: str, raw: dict[str, Any]) -> RemoteMcpServerConfig:
    type_value = str(raw.get("type", "remote")).strip().lower()
    if type_value != "remote":
        raise ValueError(f"MCP server '{name}' only supports type='remote' in v1.")

    url = str(raw.get("url") or "").strip()
    if not url:
        raise ValueError(f"MCP server '{name}' is missing a non-empty url.")

    transport = str(raw.get("transport", "auto") or "auto").strip().lower()
    if transport not in SUPPORTED_TRANSPORTS:
        raise ValueError(
            f"MCP server '{name}' has unsupported transport '{transport}'. "
            "Supported values are auto, http, sse."
        )

    enabled = bool(raw.get("enabled", True))
    headers = _normalize_headers(raw.get("headers"))
    timeout_ms = _parse_int(raw.get("timeout_ms", 30000), default=30000, minimum=1000)

    tools_raw = raw.get("tools")
    tool_filter = _parse_tool_filter(tools_raw)

    return RemoteMcpServerConfig(
        name=name,
        url=url,
        transport=transport,  # type: ignore[arg-type]
        enabled=enabled,
        headers=headers,
        timeout_ms=timeout_ms,
        tools=tool_filter,
    )


def _parse_tool_filter(raw: Any) -> McpToolFilter:
    if raw is None:
        return McpToolFilter()
    if not isinstance(raw, dict):
        raise ValueError("MCP tools filter must be an object with allow/deny arrays.")
    allow = _normalize_pattern_list(raw.get("allow"), default=("*",))
    deny = _normalize_pattern_list(raw.get("deny"), default=())
    return McpToolFilter(allow=allow, deny=deny)


def _normalize_pattern_list(raw: Any, *, default: tuple[str, ...]) -> tuple[str, ...]:
    if raw is None:
        return default
    if not isinstance(raw, list):
        raise ValueError("MCP tool filters must use string arrays.")
    items = tuple(str(item).strip() for item in raw if str(item).strip())
    return items or default


def _normalize_headers(raw: Any) -> dict[str, str]:
    if raw is None:
        return {}
    if not isinstance(raw, dict):
        raise ValueError("MCP headers must be an object.")
    headers: dict[str, str] = {}
    for key, value in raw.items():
        header = str(key).strip()
        if not header:
            continue
        headers[header] = str(value)
    return headers


def _parse_int(value: Any, *, default: int, minimum: int) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        return default
    return max(parsed, minimum)


def _parse_float(value: Any, *, default: float, minimum: float) -> float:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return default
    return max(parsed, minimum)

