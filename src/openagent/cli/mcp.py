from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any

from openagent.core.mcp import RemoteMcpManager, load_mcp_config

MCP_CONFIG_ENV = "OPENAGENT_MCP_CONFIG"
SUPPORTED_MCP_TRANSPORTS = {"auto", "http", "sse"}
SECRET_HEADER_NAMES = {"authorization", "proxy-authorization", "x-api-key", "api-key"}
REMOTE_MCP_TYPES = {"remote", "streamablehttp", "streamable_http", "http", "sse"}
AUTH_TOKEN_FIELD_KEYS = {
    "accesstoken",
    "apikey",
    "authtoken",
    "bearertoken",
    "idtoken",
    "refreshtoken",
    "token",
    "tokens",
}
SECRET_FIELD_KEYS = AUTH_TOKEN_FIELD_KEYS | {
    "clientsecret",
    "secret",
}
LOGOUT_FIELD_KEYS = SECRET_FIELD_KEYS | {
    "auth",
    "authorization",
    "client",
    "clientid",
    "oauth",
    "oauth2",
}
OAUTH_MARKER_KEYS = {
    "authorizationurl",
    "clientid",
    "clientsecret",
    "issuer",
    "oauth",
    "oauth2",
    "scope",
    "scopes",
    "tokenurl",
}


def run_mcp_command(args: argparse.Namespace, *, stdout: object | None = None, stderr: object | None = None) -> int:
    out = stdout or sys.stdout
    err = stderr or sys.stderr
    command = str(getattr(args, "mcp_command", ""))
    try:
        if command in {"list", "ls"}:
            payload = list_mcp_servers(args)
            print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="list")
            return 0
        if command == "show":
            payload = show_mcp_server(args)
            print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="show")
            return 0
        if command == "auth":
            auth_command = str(getattr(args, "mcp_auth_command", ""))
            if auth_command in {"list", "ls"}:
                payload = list_mcp_auth(args)
                print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="auth_list")
                return 0
            if auth_command == "status":
                payload = status_mcp_auth(args)
                table_kind = "auth_status" if "server" in payload else "auth_list"
                print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind=table_kind)
                return 0
            if auth_command == "set-token":
                payload = set_mcp_auth_token(args)
                print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="auth_update")
                return 0
            raise ValueError(f"Unknown MCP auth command: {auth_command}")
        if command == "add":
            payload = add_mcp_server(args)
            print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="show")
            return 0
        if command in {"remove", "rm"}:
            payload = remove_mcp_server(args)
            print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="remove")
            return 0
        if command == "logout":
            payload = logout_mcp_server(args)
            print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="auth_update")
            return 0
        if command == "doctor":
            payload = doctor_mcp(args)
            print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="doctor")
            return 0 if bool(payload.get("ok", False)) else 2
        if command == "debug":
            payload = debug_mcp_server(args)
            print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="debug")
            return 0 if bool(payload.get("ok", False)) else 2
    except (FileNotFoundError, KeyError) as error:
        print(str(error), file=err)
        return 1
    except ValueError as error:
        print(str(error), file=err)
        return 2

    print(f"Unknown MCP command: {command}", file=err)
    return 2


def list_mcp_servers(args: argparse.Namespace) -> dict[str, object]:
    path = resolve_mcp_config_path(args)
    raw = read_mcp_config_file(path)
    return {
        "config_path": str(path),
        "servers": [redact_server(name, value) for name, value in sorted(mcp_servers(raw).items())],
    }


def show_mcp_server(args: argparse.Namespace) -> dict[str, object]:
    path = resolve_mcp_config_path(args)
    raw = read_mcp_config_file(path)
    name = str(getattr(args, "name", "")).strip()
    servers = mcp_servers(raw)
    if name not in servers:
        raise KeyError(f"MCP server not found: {name}")
    return {"config_path": str(path), "server": redact_server(name, servers[name])}


def list_mcp_auth(args: argparse.Namespace) -> dict[str, object]:
    path = resolve_mcp_config_path(args)
    raw = read_mcp_config_file(path)
    return {
        "config_path": str(path),
        "servers": [mcp_auth_status_item(name, value) for name, value in sorted(mcp_servers(raw).items())],
    }


def status_mcp_auth(args: argparse.Namespace) -> dict[str, object]:
    name = str(getattr(args, "name", "") or "").strip()
    if not name:
        return list_mcp_auth(args)
    path = resolve_mcp_config_path(args)
    raw = read_mcp_config_file(path)
    servers = mcp_servers(raw)
    if name not in servers:
        raise KeyError(f"MCP server not found: {name}")
    return {"config_path": str(path), "server": mcp_auth_status_item(name, servers[name])}


def set_mcp_auth_token(args: argparse.Namespace) -> dict[str, object]:
    name = normalize_server_name(str(getattr(args, "name", "")))
    token = read_bearer_token(args)
    header_name = normalize_header_name(str(getattr(args, "header_name", "") or "Authorization"))
    path = resolve_mcp_config_path(args)
    raw = read_mcp_config_file(path)
    servers = mcp_servers(raw)
    if name not in servers:
        raise KeyError(f"MCP server not found: {name}")
    server = ensure_server_object(name, servers[name])
    status = mcp_auth_status_item(name, server)
    if status.get("status") == "not_remote":
        raise ValueError(f"MCP server is not remote: {name}")
    if status.get("status") == "error":
        raise ValueError(str(status.get("error") or f"MCP server is invalid: {name}"))
    headers = server.setdefault("headers", {})
    if not isinstance(headers, dict):
        raise ValueError(f"MCP server '{name}' headers must be an object.")
    headers[header_name] = format_token_header_value(header_name, token)
    write_mcp_config_file(path, raw)
    return {
        "config_path": str(path),
        "updated": True,
        "server": mcp_auth_status_item(name, server),
    }


def add_mcp_server(args: argparse.Namespace) -> dict[str, object]:
    name = normalize_server_name(str(getattr(args, "name", "")))
    url = str(getattr(args, "url", "") or "").strip()
    if not url:
        raise ValueError("MCP server URL is required.")
    transport = str(getattr(args, "transport", "auto") or "auto").strip().lower()
    if transport not in SUPPORTED_MCP_TRANSPORTS:
        raise ValueError("MCP transport must be one of: auto, http, sse.")
    timeout_ms = int(getattr(args, "timeout_ms", 30000) or 30000)
    if timeout_ms < 1000:
        raise ValueError("MCP timeout-ms must be at least 1000.")

    headers = parse_headers(list(getattr(args, "header", []) or []))
    path = resolve_mcp_config_path(args)
    raw = read_mcp_config_file(path)
    servers = mcp_servers(raw)
    servers[name] = {
        "type": "remote",
        "url": url,
        "transport": transport,
        "enabled": not bool(getattr(args, "disabled", False)),
        "headers": headers,
        "timeout_ms": timeout_ms,
    }
    write_mcp_config_file(path, raw)
    return {"config_path": str(path), "server": redact_server(name, servers[name]), "updated": True}


def remove_mcp_server(args: argparse.Namespace) -> dict[str, object]:
    name = str(getattr(args, "name", "")).strip()
    path = resolve_mcp_config_path(args)
    raw = read_mcp_config_file(path)
    servers = mcp_servers(raw)
    if name not in servers:
        raise KeyError(f"MCP server not found: {name}")
    del servers[name]
    write_mcp_config_file(path, raw)
    return {"config_path": str(path), "removed": True, "name": name}


def logout_mcp_server(args: argparse.Namespace) -> dict[str, object]:
    name = str(getattr(args, "name", "")).strip()
    path = resolve_mcp_config_path(args)
    raw = read_mcp_config_file(path)
    servers = mcp_servers(raw)
    if name not in servers:
        raise KeyError(f"MCP server not found: {name}")
    server = ensure_server_object(name, servers[name])
    removed_headers = remove_auth_headers(server)
    removed_fields = remove_auth_fields(server)
    write_mcp_config_file(path, raw)
    return {
        "config_path": str(path),
        "logged_out": bool(removed_headers or removed_fields),
        "removed_headers": removed_headers,
        "removed_fields": removed_fields,
        "server": mcp_auth_status_item(name, server),
    }


def doctor_mcp(args: argparse.Namespace) -> dict[str, object]:
    path = resolve_mcp_config_path(args)
    raw = read_mcp_config_file(path)
    try:
        config = load_mcp_config(raw)
    except ValueError as error:
        return {
            "config_path": str(path),
            "configured": False,
            "ok": False,
            "error": str(error),
            "servers": [],
        }

    manager = RemoteMcpManager(config)
    refresh_error: str | None = None
    if bool(getattr(args, "refresh", False)):
        try:
            manager.refresh_all_sync()
        except Exception as error:  # noqa: BLE001 - network/client failures are doctor status, not tracebacks.
            refresh_error = str(error)
    snapshot = manager.snapshot().to_dict()
    servers = []
    for server in snapshot["servers"]:
        item = dict(server)
        status = str(item.get("status") or "")
        item["ok"] = bool(item.get("enabled") is False or status not in {"error"})
        servers.append(item)
    if refresh_error and servers:
        for item in servers:
            if item.get("enabled") and item.get("status") != "error":
                item["status"] = "error"
                item["last_error"] = refresh_error
                item["ok"] = False
    return {
        "config_path": str(path),
        "configured": bool(snapshot["configured"]),
        "enabled": bool(snapshot["enabled"]),
        "server_count": int(snapshot["server_count"]),
        "ok": all(server.get("ok") for server in servers),
        "refresh_error": refresh_error,
        "servers": servers,
    }


def debug_mcp_server(args: argparse.Namespace) -> dict[str, object]:
    name = str(getattr(args, "name", "")).strip()
    path = resolve_mcp_config_path(args)
    raw = read_mcp_config_file(path)
    servers = mcp_servers(raw)
    if name not in servers:
        raise KeyError(f"MCP server not found: {name}")

    server_raw = ensure_server_object(name, servers[name])
    auth_status = mcp_auth_status_item(name, server_raw)
    payload: dict[str, object] = {
        "config_path": str(path),
        "ok": False,
        "server": auth_status,
        "headers": redacted_headers(server_raw),
        "config_status": {"ok": False, "error": None},
        "runtime_status": None,
        "refresh_error": None,
    }

    if auth_status.get("status") == "not_remote":
        payload["config_status"] = {"ok": False, "error": f"MCP server is not remote: {name}"}
        return payload

    scoped_raw: dict[str, Any] = {"mcpServers": {name: server_raw}}
    if "refresh_ttl_s" in raw:
        scoped_raw["refresh_ttl_s"] = raw["refresh_ttl_s"]
    try:
        config = load_mcp_config(scoped_raw)
    except ValueError as error:
        payload["config_status"] = {"ok": False, "error": str(error)}
        return payload

    payload["config_status"] = {"ok": True, "error": None}
    manager = RemoteMcpManager(config)
    refresh_error: str | None = None
    if bool(getattr(args, "refresh", False)):
        try:
            manager.refresh_all_sync()
        except Exception as error:  # noqa: BLE001 - debug reports network/client failures structurally.
            refresh_error = str(error)

    runtime_server = next(
        (dict(item) for item in manager.snapshot().to_dict()["servers"] if item.get("name") == name),
        None,
    )
    if runtime_server is None:
        runtime_server = {"name": name, "status": "error", "last_error": "MCP server missing from runtime snapshot.", "ok": False}
    else:
        runtime_server["ok"] = bool(runtime_server.get("enabled") is False or runtime_server.get("status") != "error")
    if refresh_error:
        runtime_server["status"] = "error"
        runtime_server["last_error"] = refresh_error
        runtime_server["ok"] = False

    payload["runtime_status"] = runtime_server
    payload["refresh_error"] = refresh_error
    payload["ok"] = bool(
        payload["config_status"]
        and isinstance(payload["config_status"], dict)
        and payload["config_status"].get("ok")
        and runtime_server.get("ok")
        and auth_status.get("status") not in {"error", "not_remote", "needs_auth"}
    )
    return payload


def resolve_mcp_config_path(args: argparse.Namespace) -> Path:
    workspace = Path(getattr(args, "workspace", None) or Path.cwd()).expanduser().resolve()
    explicit = getattr(args, "mcp_config", None) or getattr(args, "config", None)
    source = explicit or os.getenv(MCP_CONFIG_ENV)
    if source:
        text = str(source).strip()
        if text.startswith("{"):
            raise ValueError(f"{MCP_CONFIG_ENV} must point to a JSON file when using openagent mcp.")
        path = Path(text).expanduser()
        return path if path.is_absolute() else (workspace / path).resolve()
    return workspace / ".openagent" / "mcp.json"


def read_mcp_config_file(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {"mcpServers": {}}
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise ValueError(f"MCP config file is not valid JSON: {path}") from error
    if not isinstance(raw, dict):
        raise ValueError("MCP config must be a JSON object.")
    if "mcpServers" not in raw:
        if "mcp" in raw and isinstance(raw["mcp"], dict):
            raw["mcpServers"] = raw.pop("mcp")
        else:
            raw["mcpServers"] = {}
    if not isinstance(raw["mcpServers"], dict):
        raise ValueError("MCP config must contain an object-valued 'mcpServers' field.")
    return raw


def write_mcp_config_file(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    try:
        os.chmod(path, 0o600)
    except OSError:
        pass


def mcp_servers(payload: dict[str, Any]) -> dict[str, Any]:
    servers = payload.setdefault("mcpServers", {})
    if not isinstance(servers, dict):
        raise ValueError("MCP config must contain an object-valued 'mcpServers' field.")
    return servers


def normalize_server_name(name: str) -> str:
    normalized = name.strip()
    if not normalized or "/" in normalized or normalized in {".", ".."}:
        raise ValueError(f"Invalid MCP server name: {name}")
    return normalized


def parse_headers(values: list[str]) -> dict[str, str]:
    headers: dict[str, str] = {}
    for raw in values:
        if "=" not in raw:
            raise ValueError("MCP headers must use KEY=VALUE.")
        key, value = raw.split("=", 1)
        key = key.strip()
        if not key:
            raise ValueError("MCP header key cannot be empty.")
        headers[key] = value
    return headers


def read_bearer_token(args: argparse.Namespace) -> str:
    token = getattr(args, "bearer_token", None)
    if token is None and bool(getattr(args, "bearer_token_stdin", False)):
        token = sys.stdin.read()
    value = str(token or "").strip()
    if not value:
        raise ValueError("MCP bearer token cannot be empty.")
    return value


def normalize_header_name(name: str) -> str:
    header = name.strip()
    if not header:
        raise ValueError("MCP auth header name cannot be empty.")
    if any(char in header for char in "\r\n:"):
        raise ValueError("MCP auth header name cannot contain ':' or newlines.")
    return header


def format_token_header_value(header_name: str, token: str) -> str:
    if header_name.strip().lower() in {"authorization", "proxy-authorization"}:
        return token if token.lower().startswith("bearer ") else f"Bearer {token}"
    return token


def ensure_server_object(name: str, raw: Any) -> dict[str, Any]:
    if not isinstance(raw, dict):
        raise ValueError(f"MCP server '{name}' must be configured with an object.")
    return raw


def redact_server(name: str, raw: Any) -> dict[str, object]:
    value = raw if isinstance(raw, dict) else {}
    headers = value.get("headers") if isinstance(value.get("headers"), dict) else {}
    header_names = sorted(str(key) for key in headers)
    return {
        "name": name,
        "url": str(value.get("url") or ""),
        "transport": str(value.get("transport") or infer_transport(value)),
        "enabled": bool(value.get("enabled", True)),
        "timeout_ms": int(value.get("timeout_ms") or 30000),
        "header_names": header_names,
        "headers": {header: redact_header_value(header) for header in header_names},
    }


def mcp_auth_status_item(name: str, raw: Any) -> dict[str, object]:
    if not isinstance(raw, dict):
        return {
            "name": name,
            "url": "",
            "transport": "auto",
            "enabled": False,
            "remote": False,
            "oauth_capable": False,
            "oauth_enabled": False,
            "status": "error",
            "error": f"MCP server '{name}' must be configured with an object.",
            "header_names": [],
            "auth_header_names": [],
            "token_fields": [],
            "secret_field_count": 0,
        }

    headers = raw.get("headers") if isinstance(raw.get("headers"), dict) else {}
    header_names = sorted(str(key) for key in headers)
    auth_header_names = [header for header in header_names if is_auth_header_name(header)]
    token_fields = collect_field_paths(raw, AUTH_TOKEN_FIELD_KEYS)
    secret_field_count = len(collect_field_paths(raw, SECRET_FIELD_KEYS))
    remote, remote_error = inspect_remote_server(raw)
    oauth_capable, oauth_enabled = inspect_oauth_state(raw)
    enabled = bool(raw.get("enabled", True))

    if not enabled:
        status = "disabled"
        error = None
    elif not remote:
        status = "not_remote"
        error = None
    elif remote_error:
        status = "error"
        error = remote_error
    elif auth_header_names or token_fields:
        status = "authenticated"
        error = None
    elif oauth_capable and oauth_enabled:
        status = "needs_auth"
        error = None
    elif oauth_capable and not oauth_enabled:
        status = "oauth_disabled"
        error = None
    else:
        status = "not_authenticated"
        error = None

    return {
        "name": name,
        "url": str(raw.get("url") or ""),
        "transport": str(raw.get("transport") or infer_transport(raw)),
        "enabled": enabled,
        "remote": remote,
        "oauth_capable": oauth_capable,
        "oauth_enabled": oauth_enabled,
        "status": status,
        "error": error,
        "header_names": header_names,
        "auth_header_names": auth_header_names,
        "token_fields": token_fields,
        "secret_field_count": secret_field_count,
    }


def inspect_remote_server(value: dict[str, Any]) -> tuple[bool, str | None]:
    type_value = str(value.get("type") or "").strip().lower()
    if type_value and type_value not in REMOTE_MCP_TYPES:
        return False, None
    url = str(value.get("url") or "").strip()
    if not url:
        if "command" in value and not type_value:
            return False, None
        return True, "MCP remote server is missing a non-empty url."
    return True, None


def inspect_oauth_state(value: dict[str, Any]) -> tuple[bool, bool]:
    for key, raw in value.items():
        if normalize_config_key(key) in {"oauth", "oauth2"}:
            if raw is False or raw is None:
                return True, False
            if isinstance(raw, dict):
                return True, bool(raw.get("enabled", True))
            return True, bool(raw)
    marker_found = any(normalize_config_key(key) in OAUTH_MARKER_KEYS for key in value)
    return marker_found, marker_found


def collect_field_paths(value: Any, keys: set[str], *, prefix: str = "") -> list[str]:
    paths: list[str] = []
    if isinstance(value, dict):
        for raw_key, child in value.items():
            key = str(raw_key)
            if not prefix and key == "headers":
                continue
            path = f"{prefix}.{key}" if prefix else key
            if normalize_config_key(key) in keys:
                paths.append(path)
                continue
            paths.extend(collect_field_paths(child, keys, prefix=path))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            path = f"{prefix}[{index}]" if prefix else f"[{index}]"
            paths.extend(collect_field_paths(child, keys, prefix=path))
    return sorted(paths)


def remove_auth_headers(server: dict[str, Any]) -> list[str]:
    headers = server.get("headers")
    if headers is None:
        return []
    if not isinstance(headers, dict):
        raise ValueError("MCP headers must be an object.")
    removed: list[str] = []
    for key in list(headers):
        header = str(key)
        if is_auth_header_name(header):
            removed.append(header)
            del headers[key]
    return sorted(removed)


def remove_auth_fields(value: Any, *, prefix: str = "") -> list[str]:
    removed: list[str] = []
    if not isinstance(value, dict):
        return removed
    for raw_key in list(value):
        key = str(raw_key)
        if not prefix and key == "headers":
            continue
        path = f"{prefix}.{key}" if prefix else key
        if normalize_config_key(key) in LOGOUT_FIELD_KEYS:
            removed.append(path)
            del value[raw_key]
            continue
        removed.extend(remove_auth_fields(value[raw_key], prefix=path))
    return sorted(removed)


def redacted_headers(value: dict[str, Any]) -> dict[str, str]:
    headers = value.get("headers") if isinstance(value.get("headers"), dict) else {}
    return {str(header): redact_header_value(str(header)) for header in sorted(headers)}


def is_auth_header_name(header: str) -> bool:
    lower = header.strip().lower()
    normalized = normalize_config_key(header)
    return (
        lower in SECRET_HEADER_NAMES
        or normalized in {"authorization", "proxyauthorization", "xapikey", "apikey"}
        or "token" in normalized
        or normalized.endswith("apikey")
    )


def normalize_config_key(key: object) -> str:
    return "".join(char for char in str(key).lower() if char.isalnum())


def infer_transport(value: dict[str, Any]) -> str:
    type_value = str(value.get("type") or "remote").lower()
    if type_value in {"streamablehttp", "streamable_http", "http"}:
        return "http"
    if type_value == "sse":
        return "sse"
    return "auto"


def redact_header_value(header: str) -> str:
    if header.lower() in SECRET_HEADER_NAMES:
        return "[redacted]"
    return "[redacted]"


def print_mcp_payload(payload: dict[str, object], *, output_format: str, stdout: object, table_kind: str) -> None:
    if output_format == "json":
        print(json.dumps(payload, ensure_ascii=False, sort_keys=True), file=stdout)
        return
    if table_kind == "list":
        print_mcp_list_table(payload, stdout=stdout)
    elif table_kind in {"auth_list", "auth_status"}:
        print_mcp_auth_table(payload, stdout=stdout)
    elif table_kind == "auth_update":
        print_mcp_auth_update_table(payload, stdout=stdout)
    elif table_kind == "doctor":
        print_mcp_doctor_table(payload, stdout=stdout)
    elif table_kind == "debug":
        print_mcp_debug_table(payload, stdout=stdout)
    elif table_kind == "remove":
        print(f"removed: {payload.get('name')}", file=stdout)
        print(f"config: {payload.get('config_path')}", file=stdout)
    else:
        server = payload.get("server") if isinstance(payload.get("server"), dict) else {}
        print(f"name: {server.get('name') or ''}", file=stdout)
        print(f"url: {server.get('url') or ''}", file=stdout)
        print(f"transport: {server.get('transport') or ''}", file=stdout)
        print(f"enabled: {server.get('enabled')}", file=stdout)
        print(f"timeout_ms: {server.get('timeout_ms')}", file=stdout)
        print(f"headers: {', '.join(server.get('header_names') or [])}", file=stdout)
        print(f"config: {payload.get('config_path')}", file=stdout)


def print_mcp_list_table(payload: dict[str, object], *, stdout: object) -> None:
    servers = payload.get("servers") if isinstance(payload.get("servers"), list) else []
    if not servers:
        print("No MCP servers configured.", file=stdout)
        return
    rows = [["name", "enabled", "transport", "timeout_ms", "headers", "url"]]
    for server in servers:
        item = server if isinstance(server, dict) else {}
        rows.append(
            [
                str(item.get("name") or ""),
                str(item.get("enabled")),
                str(item.get("transport") or ""),
                str(item.get("timeout_ms") or ""),
                ",".join(str(name) for name in item.get("header_names") or []),
                str(item.get("url") or ""),
            ]
        )
    print_table(rows, stdout=stdout)


def print_mcp_auth_table(payload: dict[str, object], *, stdout: object) -> None:
    if isinstance(payload.get("server"), dict):
        servers = [payload["server"]]
    else:
        servers = payload.get("servers") if isinstance(payload.get("servers"), list) else []
    if not servers:
        print("No MCP servers configured.", file=stdout)
        return
    rows = [["name", "enabled", "remote", "oauth", "status", "auth_headers", "token_fields", "url"]]
    for server in servers:
        item = server if isinstance(server, dict) else {}
        rows.append(
            [
                str(item.get("name") or ""),
                str(item.get("enabled")),
                str(item.get("remote")),
                str(item.get("oauth_capable")),
                str(item.get("status") or ""),
                ",".join(str(name) for name in item.get("auth_header_names") or []),
                ",".join(str(name) for name in item.get("token_fields") or []),
                str(item.get("url") or ""),
            ]
        )
    print_table(rows, stdout=stdout)
    print(f"config: {payload.get('config_path')}", file=stdout)


def print_mcp_auth_update_table(payload: dict[str, object], *, stdout: object) -> None:
    server = payload.get("server") if isinstance(payload.get("server"), dict) else {}
    print(f"name: {server.get('name') or ''}", file=stdout)
    print(f"auth_status: {server.get('status') or ''}", file=stdout)
    print(f"auth_headers: {', '.join(server.get('auth_header_names') or [])}", file=stdout)
    if "logged_out" in payload:
        print(f"logged_out: {payload.get('logged_out')}", file=stdout)
        print(f"removed_headers: {', '.join(payload.get('removed_headers') or [])}", file=stdout)
        print(f"removed_fields: {', '.join(payload.get('removed_fields') or [])}", file=stdout)
    else:
        print(f"updated: {payload.get('updated')}", file=stdout)
    print(f"config: {payload.get('config_path')}", file=stdout)


def print_mcp_doctor_table(payload: dict[str, object], *, stdout: object) -> None:
    if payload.get("error"):
        print(f"config: failed ({payload.get('error')})", file=stdout)
        return
    servers = payload.get("servers") if isinstance(payload.get("servers"), list) else []
    print(f"config: {payload.get('config_path')}", file=stdout)
    print(f"configured: {payload.get('configured')}", file=stdout)
    if not servers:
        print("No MCP servers configured.", file=stdout)
        return
    rows = [["name", "enabled", "status", "tools", "transport", "error"]]
    for server in servers:
        item = server if isinstance(server, dict) else {}
        rows.append(
            [
                str(item.get("name") or ""),
                str(item.get("enabled")),
                str(item.get("status") or ""),
                str(item.get("tool_count") or 0),
                str(item.get("selected_transport") or item.get("configured_transport") or ""),
                str(item.get("last_error") or ""),
            ]
        )
    print_table(rows, stdout=stdout)


def print_mcp_debug_table(payload: dict[str, object], *, stdout: object) -> None:
    server = payload.get("server") if isinstance(payload.get("server"), dict) else {}
    config_status = payload.get("config_status") if isinstance(payload.get("config_status"), dict) else {}
    runtime_status = payload.get("runtime_status") if isinstance(payload.get("runtime_status"), dict) else {}
    print(f"config: {payload.get('config_path')}", file=stdout)
    print(f"name: {server.get('name') or ''}", file=stdout)
    print(f"remote: {server.get('remote')}", file=stdout)
    print(f"oauth_capable: {server.get('oauth_capable')}", file=stdout)
    print(f"auth_status: {server.get('status') or ''}", file=stdout)
    print(f"headers: {', '.join(server.get('header_names') or [])}", file=stdout)
    print(f"auth_headers: {', '.join(server.get('auth_header_names') or [])}", file=stdout)
    print(f"config_ok: {config_status.get('ok')}", file=stdout)
    print(f"config_error: {config_status.get('error') or ''}", file=stdout)
    print(f"runtime_status: {runtime_status.get('status') or ''}", file=stdout)
    print(f"runtime_error: {runtime_status.get('last_error') or ''}", file=stdout)
    print(f"ok: {payload.get('ok')}", file=stdout)


def print_table(rows: list[list[str]], *, stdout: object) -> None:
    widths = [max(len(row[index]) for row in rows) for index in range(len(rows[0]))]
    for row_index, row in enumerate(rows):
        print("  ".join(value.ljust(widths[index]) for index, value in enumerate(row)).rstrip(), file=stdout)
        if row_index == 0:
            print("  ".join("-" * width for width in widths).rstrip(), file=stdout)
