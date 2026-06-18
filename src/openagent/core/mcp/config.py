from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from .types import McpConfig, McpOAuthClientInfo, McpOAuthConfig, McpOAuthTokens, McpToolFilter, RemoteMcpServerConfig

SUPPORTED_TRANSPORTS = {"auto", "http", "sse"}
STREAMABLE_HTTP_TYPES = {"streamablehttp", "streamable_http", "http"}


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

    mcp_block = raw.get("mcpServers", raw.get("mcp", raw))
    if not isinstance(mcp_block, dict):
        raise ValueError("MCP config must contain an object-valued 'mcp' or 'mcpServers' field.")

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
    default_transport = "auto"
    if type_value in STREAMABLE_HTTP_TYPES:
        default_transport = "http"
    elif type_value == "sse":
        default_transport = "sse"
    elif type_value != "remote":
        raise ValueError(
            f"MCP server '{name}' only supports type='remote', 'streamableHttp', or 'sse' in v1."
        )

    url = str(raw.get("url") or "").strip()
    if not url:
        raise ValueError(f"MCP server '{name}' is missing a non-empty url.")

    transport = str(raw.get("transport", default_transport) or default_transport).strip().lower()
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
    oauth = _parse_oauth_config(name, raw.get("oauth"))

    return RemoteMcpServerConfig(
        name=name,
        url=url,
        transport=transport,  # type: ignore[arg-type]
        enabled=enabled,
        headers=headers,
        timeout_ms=timeout_ms,
        tools=tool_filter,
        oauth=oauth,
    )


def _parse_oauth_config(name: str, raw: Any) -> McpOAuthConfig | None:
    if raw is None:
        return None
    if raw is False:
        return McpOAuthConfig(enabled=False)
    if raw is True:
        raw = {}
    if not isinstance(raw, dict):
        raise ValueError(f"MCP server '{name}' oauth config must be a boolean or object.")

    enabled = bool(raw.get("enabled", True))
    redirect_uris = _parse_redirect_uris(
        name,
        raw.get("redirect_uris", raw.get("redirectUris", raw.get("redirect_uri", raw.get("redirectUri")))),
    )
    scopes = _parse_scopes(name, raw.get("scopes", raw.get("scope")))
    client_name = str(raw.get("client_name", raw.get("clientName", "OpenAgent")) or "OpenAgent").strip() or "OpenAgent"
    client_uri = _optional_url(name, raw.get("client_uri", raw.get("clientUri")), field="client_uri")
    client_metadata_url = _optional_url(
        name,
        raw.get("client_metadata_url", raw.get("clientMetadataUrl")),
        field="client_metadata_url",
        require_https=True,
        require_path=True,
    )
    timeout_s = _parse_float(raw.get("timeout_s", raw.get("timeout", 300.0)), default=300.0, minimum=1.0)

    return McpOAuthConfig(
        enabled=enabled,
        redirect_uris=redirect_uris,
        scopes=scopes,
        client_name=client_name,
        client_uri=client_uri,
        client_metadata_url=client_metadata_url,
        timeout_s=timeout_s,
        tokens=_parse_oauth_tokens(raw.get("tokens") if isinstance(raw.get("tokens"), dict) else raw),
        client=_parse_oauth_client_info(raw, redirect_uris=redirect_uris),
    )


def _parse_oauth_tokens(raw: Any) -> McpOAuthTokens | None:
    if not isinstance(raw, dict):
        return None
    access_token = _first_present(raw, "access_token", "accessToken", "token", "bearer_token", "bearerToken")
    if not access_token:
        return None
    refresh_token = _first_present(raw, "refresh_token", "refreshToken")
    token_type = str(_first_present(raw, "token_type", "tokenType") or "Bearer").strip() or "Bearer"
    expires_in = _optional_int(_first_present(raw, "expires_in", "expiresIn"))
    scope = _scope_string(raw.get("scope", raw.get("scopes")))
    return McpOAuthTokens(
        access_token=str(access_token),
        token_type=token_type,
        expires_in=expires_in,
        scope=scope,
        refresh_token=str(refresh_token) if refresh_token else None,
    )


def _parse_oauth_client_info(raw: dict[str, Any], *, redirect_uris: tuple[str, ...]) -> McpOAuthClientInfo | None:
    client_raw = raw.get("client") if isinstance(raw.get("client"), dict) else {}
    source: dict[str, Any] = {}
    if isinstance(client_raw, dict):
        source.update(client_raw)
    for key in (
        "client_id",
        "clientId",
        "client_secret",
        "clientSecret",
        "client_id_issued_at",
        "clientIdIssuedAt",
        "client_secret_expires_at",
        "clientSecretExpiresAt",
        "token_endpoint_auth_method",
        "tokenEndpointAuthMethod",
    ):
        if key in raw and key not in source:
            source[key] = raw[key]
    client_id = _first_present(source, "client_id", "clientId")
    client_secret = _first_present(source, "client_secret", "clientSecret")
    if not client_id and not client_secret:
        return None
    client_redirects = _parse_redirect_uris(
        "oauth client",
        source.get("redirect_uris", source.get("redirectUris", source.get("redirect_uri", source.get("redirectUri")))),
        default=redirect_uris,
    )
    return McpOAuthClientInfo(
        client_id=str(client_id) if client_id else None,
        client_secret=str(client_secret) if client_secret else None,
        client_id_issued_at=_optional_int(_first_present(source, "client_id_issued_at", "clientIdIssuedAt")),
        client_secret_expires_at=_optional_int(
            _first_present(source, "client_secret_expires_at", "clientSecretExpiresAt")
        ),
        redirect_uris=client_redirects,
        token_endpoint_auth_method=(
            str(_first_present(source, "token_endpoint_auth_method", "tokenEndpointAuthMethod"))
            if _first_present(source, "token_endpoint_auth_method", "tokenEndpointAuthMethod")
            else None
        ),
    )


def _parse_redirect_uris(name: str, raw: Any, *, default: tuple[str, ...] | None = None) -> tuple[str, ...]:
    if raw is None:
        return default or ("http://127.0.0.1:14555/oauth/callback",)
    values = _normalize_string_list(raw, field="redirect_uris")
    if not values:
        raise ValueError(f"MCP server '{name}' oauth redirect_uris cannot be empty.")
    for value in values:
        parsed = urlparse(value)
        if parsed.scheme not in {"http", "https"} or not parsed.netloc:
            raise ValueError(f"MCP server '{name}' oauth redirect_uri must be an absolute http(s) URL.")
    return values


def _parse_scopes(name: str, raw: Any) -> tuple[str, ...]:
    if raw is None:
        return ()
    values = _normalize_string_list(raw, field="scopes")
    for value in values:
        if any(char.isspace() for char in value):
            raise ValueError(f"MCP server '{name}' oauth scopes must not contain whitespace.")
    return values


def _scope_string(raw: Any) -> str | None:
    values = _normalize_string_list(raw, field="scope") if raw is not None else ()
    return " ".join(values) if values else None


def _normalize_string_list(raw: Any, *, field: str) -> tuple[str, ...]:
    if raw is None:
        return ()
    if isinstance(raw, str):
        items = raw.split() if field in {"scope", "scopes"} else [raw]
    elif isinstance(raw, list):
        items = [str(item) for item in raw]
    else:
        raise ValueError(f"MCP oauth {field} must be a string or string array.")
    return tuple(item.strip() for item in items if item.strip())


def _optional_url(
    name: str,
    raw: Any,
    *,
    field: str,
    require_https: bool = False,
    require_path: bool = False,
) -> str | None:
    if raw is None or str(raw).strip() == "":
        return None
    value = str(raw).strip()
    parsed = urlparse(value)
    if not parsed.scheme or not parsed.netloc:
        raise ValueError(f"MCP server '{name}' oauth {field} must be an absolute URL.")
    if require_https and parsed.scheme != "https":
        raise ValueError(f"MCP server '{name}' oauth {field} must use https.")
    if require_path and parsed.path in {"", "/"}:
        raise ValueError(f"MCP server '{name}' oauth {field} must include a non-root path.")
    return value


def _first_present(raw: dict[str, Any], *keys: str) -> Any:
    for key in keys:
        value = raw.get(key)
        if value is not None and str(value).strip() != "":
            return value
    return None


def _optional_int(value: Any) -> int | None:
    if value is None or str(value).strip() == "":
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


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
