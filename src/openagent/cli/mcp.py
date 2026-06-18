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
        if command == "add":
            payload = add_mcp_server(args)
            print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="show")
            return 0
        if command in {"remove", "rm"}:
            payload = remove_mcp_server(args)
            print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="remove")
            return 0
        if command == "doctor":
            payload = doctor_mcp(args)
            print_mcp_payload(payload, output_format=str(getattr(args, "format", "table")), stdout=out, table_kind="doctor")
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
    elif table_kind == "doctor":
        print_mcp_doctor_table(payload, stdout=stdout)
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


def print_table(rows: list[list[str]], *, stdout: object) -> None:
    widths = [max(len(row[index]) for row in rows) for index in range(len(rows[0]))]
    for row_index, row in enumerate(rows):
        print("  ".join(value.ljust(widths[index]) for index, value in enumerate(row)).rstrip(), file=stdout)
        if row_index == 0:
            print("  ".join("-" * width for width in widths).rstrip(), file=stdout)
