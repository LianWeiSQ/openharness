from __future__ import annotations

import argparse
import json
import os
import secrets
import shutil
import sys
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path

from openagent.cli.auth import list_providers, load_auth_env, login_provider, logout_provider, resolve_auth_file
from openagent.cli.custom_commands import discover_commands, render_command, resolve_command
from openagent.core.provider.metadata import known_provider_ids, normalize_provider, provider_auth_methods

DEFAULT_BASE_URL = "http://localhost:8080"
DEFAULT_MODEL = "gpt-5.5"
DEFAULT_WIRE_API = "responses"
DEFAULT_MAX_STEPS = "30"
DEFAULT_SERVER_URL = "http://127.0.0.1:8787"
DEFAULT_SERVER_TOKEN_ENV = "OPENAGENT_SERVER_TOKEN"


@dataclass(frozen=True, slots=True)
class OpenAgentCliDefaults:
    base_url: str = DEFAULT_BASE_URL
    model: str = DEFAULT_MODEL
    wire_api: str = DEFAULT_WIRE_API
    max_steps: str = DEFAULT_MAX_STEPS


@dataclass(frozen=True, slots=True)
class DoctorReport:
    base_url: str
    model: str
    wire_api: str
    api_key_set: bool
    model_endpoint_ok: bool
    model_endpoint_message: str


def main(argv: list[str] | None = None) -> None:
    parser = build_parser()
    args = parser.parse_args(argv)
    command = args.command or "tui"
    if command == "mcp":
        raise SystemExit(run_mcp_command(args))
    load_local_env(getattr(args, "config", None))
    if command in {"auth", "providers"}:
        raise SystemExit(run_auth_command(args))
    load_auth_env(getattr(args, "auth_file", None))

    if command == "doctor":
        apply_model_env(args)
        raise SystemExit(run_doctor_command(args))
    if command == "serve":
        apply_model_env(args)
        run_serve(args)
        return
    if command == "web":
        apply_model_env(args)
        run_web(args)
        return
    if command == "client":
        raise SystemExit(run_client_command(args))
    if command == "attach":
        raise SystemExit(run_attach_command(args))
    if command == "run":
        apply_model_env(args)
        if not args.skip_doctor and not doctor(verbose=True):
            print("\nGateway check failed. Start your local OpenAI-compatible service, or rerun with --skip-doctor.", file=sys.stderr)
            raise SystemExit(2)
        raise SystemExit(run_non_interactive(args))
    if command == "session":
        raise SystemExit(run_session_command(args))
    if command == "models":
        apply_model_env(args)
        raise SystemExit(run_models_command(args))
    if command == "stats":
        raise SystemExit(run_stats_command(args))
    if command == "command":
        raise SystemExit(run_custom_command(args))
    if command == "config":
        raise SystemExit(run_config_command(args))
    if command == "tui":
        apply_model_env(args)
        if not args.skip_doctor and not doctor(verbose=True):
            print("\nGateway check failed. Start your local OpenAI-compatible service, or rerun with --skip-doctor.", file=sys.stderr)
            raise SystemExit(2)
        run_tui(args)
        return

    parser.error(f"unknown command: {command}")


def build_doctor_report() -> DoctorReport:
    base_url = os.getenv("OPENAI_BASE_URL") or DEFAULT_BASE_URL
    model = os.getenv("OPENAI_MODEL") or DEFAULT_MODEL
    wire_api = os.getenv("OPENAI_WIRE_API") or DEFAULT_WIRE_API
    api_key_set = bool(os.getenv("OPENAI_API_KEY"))
    models_ok, models_message = check_models_endpoint(base_url=base_url)
    return DoctorReport(
        base_url=base_url,
        model=model,
        wire_api=wire_api,
        api_key_set=api_key_set,
        model_endpoint_ok=models_ok,
        model_endpoint_message=models_message,
    )


def doctor_report_to_dict(report: DoctorReport) -> dict[str, object]:
    return {
        "base_url": report.base_url,
        "model": report.model,
        "wire_api": report.wire_api,
        "api_key_set": report.api_key_set,
        "model_endpoint_ok": report.model_endpoint_ok,
        "model_endpoint_message": report.model_endpoint_message,
    }


def print_doctor_report(report: DoctorReport, *, output_format: str = "text", stdout: object | None = None) -> None:
    if stdout is None:
        stdout = sys.stdout
    if output_format == "json":
        print(json.dumps(doctor_report_to_dict(report), sort_keys=True), file=stdout)
        return
    if output_format != "text":
        raise ValueError(f"unsupported doctor output format: {output_format}")

    print("OpenAgent doctor", file=stdout)
    print(f"- OPENAI_BASE_URL: {report.base_url}", file=stdout)
    print(f"- OPENAI_MODEL: {report.model}", file=stdout)
    print(f"- OPENAI_WIRE_API: {report.wire_api}", file=stdout)
    print(f"- OPENAI_API_KEY: {'set' if report.api_key_set else 'missing'}", file=stdout)
    print(
        f"- model endpoint: {'ok' if report.model_endpoint_ok else 'failed'} ({report.model_endpoint_message})",
        file=stdout,
    )


def doctor(*, verbose: bool = False, output_format: str = "text", stdout: object | None = None) -> bool:
    report = build_doctor_report()
    if verbose:
        print_doctor_report(report, output_format=output_format, stdout=stdout)
    return report.model_endpoint_ok


def run_doctor_command(args: argparse.Namespace, *, stdout: object | None = None) -> int:
    ok = doctor(verbose=True, output_format=getattr(args, "format", "text") or "text", stdout=stdout)
    return 0 if ok else 2


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="openagent",
        description="Start OpenAgent like a local coding agent command.",
    )
    subparsers = parser.add_subparsers(dest="command")

    add_common_model_options(parser)
    add_tui_options(parser)

    tui = subparsers.add_parser("tui", help="start the terminal UI")
    add_common_model_options(tui)
    add_tui_options(tui)

    serve_parser = subparsers.add_parser("serve", help="start the local App Bridge HTTP server")
    add_common_model_options(serve_parser)
    add_server_options(serve_parser)
    serve_parser.add_argument("--headless", action="store_true", help="serve API/SSE endpoints without the static console")
    add_server_auth_options(serve_parser, role="server")

    web = subparsers.add_parser("web", help="start the browser console")
    add_common_model_options(web)
    add_server_options(web)
    add_server_auth_options(web, role="server")

    client = subparsers.add_parser("client", help="send a prompt to a running App Bridge server")
    client.add_argument("message", nargs="*", help="prompt text")
    client.add_argument("--server-url", default=None, help=f"App Bridge URL, default OPENAGENT_SERVER_URL or {DEFAULT_SERVER_URL}")
    add_server_auth_options(client, role="client")
    client.add_argument("--workspace", "--dir", dest="workspace", default=None, help="workspace path for local files and command rendering")
    client.add_argument("--session", "-s", default=None, help="session id to continue on the server")
    client.add_argument("--continue", "-c", dest="continue_last", action="store_true", help="continue the most recent server session")
    client.add_argument("--file", "-f", action="append", default=[], help="file to attach to the prompt; can be used more than once")
    client.add_argument("--command", dest="custom_command", default=None, help="custom command name from .openagent/commands or ~/.config/openagent/commands")
    client.add_argument("--command-dir", action="append", default=[], help="extra custom command directory; can be used more than once")
    client.add_argument("--no-command-shell", action="store_true", help="render custom command without executing !`shell` blocks")
    client.add_argument("--format", choices=["text", "json"], default="text", help="output format")
    client.add_argument("--verbose", action="store_true", help="show non-answer runtime events in text mode")

    attach = subparsers.add_parser("attach", help="attach the terminal UI to a running App Bridge server")
    attach.add_argument("url", help=f"App Bridge URL, for example {DEFAULT_SERVER_URL}")
    attach.add_argument("--workspace", "--dir", dest="workspace", default=None, help="workspace path for local file mentions")
    attach.add_argument("--session", "-s", default=None, help="session id to open in the attached TUI")
    attach.add_argument("--continue", "-c", dest="continue_last", action="store_true", help="open the most recent server session")
    attach.add_argument("--skip-health-check", action="store_true", help="start TUI without checking the App Bridge first")
    add_server_auth_options(attach, role="client")

    run = subparsers.add_parser("run", help="run a prompt without launching the TUI")
    add_common_model_options(run)
    run.add_argument("message", nargs="*", help="prompt text")
    run.add_argument("--workspace", "--dir", dest="workspace", default=None, help="workspace path, default current directory")
    run.add_argument("--session-root", default=None, help="session store root")
    run.add_argument("--continue", "-c", dest="continue_last", action="store_true", help="continue the most recent session")
    run.add_argument("--session", "-s", default=None, help="session id to continue")
    run.add_argument("--file", "-f", action="append", default=[], help="file to attach to the prompt; can be used more than once")
    run.add_argument("--command", dest="custom_command", default=None, help="custom command name from .openagent/commands or ~/.config/openagent/commands")
    run.add_argument("--command-dir", action="append", default=[], help="extra custom command directory; can be used more than once")
    run.add_argument("--no-command-shell", action="store_true", help="render custom command without executing !`shell` blocks")
    run.add_argument("--format", choices=["text", "json"], default="text", help="output format")
    run.add_argument("--verbose", action="store_true", help="show non-answer runtime events in text mode")
    run.add_argument("--skip-doctor", action="store_true", help="run without checking the local model gateway first")

    session = subparsers.add_parser("session", help="manage stored sessions")
    session_subparsers = session.add_subparsers(dest="session_command", required=True)

    session_list = session_subparsers.add_parser("list", aliases=["ls"], help="list stored sessions")
    add_session_store_options(session_list)
    session_list.add_argument("--max-count", "-n", type=int, default=20, help="maximum sessions to show")
    session_list.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    session_export = session_subparsers.add_parser("export", help="export one stored session as JSON")
    add_session_store_options(session_export)
    session_export.add_argument("session_id", help="session id to export")
    session_export.add_argument("--sanitize", action="store_true", help="redact message content and local paths")

    session_delete = session_subparsers.add_parser("delete", aliases=["rm"], help="delete one stored session")
    add_session_store_options(session_delete)
    session_delete.add_argument("session_id", help="session id to delete")

    models = subparsers.add_parser("models", help="list configured model metadata")
    add_common_model_options(models)
    models.add_argument("provider", nargs="?", default=None, help="optional provider id filter")
    models.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    stats = subparsers.add_parser("stats", help="show local session usage statistics")
    add_session_store_options(stats)
    stats.add_argument("--days", type=int, default=None, help="only include sessions updated in the last N days")
    stats.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    command_parser = subparsers.add_parser("command", help="manage custom prompt commands")
    command_subparsers = command_parser.add_subparsers(dest="custom_command_action", required=True)

    command_list = command_subparsers.add_parser("list", aliases=["ls"], help="list custom commands")
    add_command_options(command_list)
    command_list.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    command_show = command_subparsers.add_parser("show", help="show one custom command")
    add_command_options(command_show)
    command_show.add_argument("name", help="command name")
    command_show.add_argument("--format", choices=["text", "json"], default="text", help="output format")

    command_render = command_subparsers.add_parser("render", help="render a custom command without running the agent")
    add_command_options(command_render)
    command_render.add_argument("name", help="command name")
    command_render.add_argument("arguments", nargs="*", help="command arguments")
    command_render.add_argument("--no-shell", action="store_true", help="do not execute !`shell` blocks")
    command_render.add_argument("--format", choices=["text", "json"], default="text", help="output format")

    config = subparsers.add_parser("config", help="inspect and initialize local CLI configuration")
    config_subparsers = config.add_subparsers(dest="config_command", required=True)

    config_init = config_subparsers.add_parser("init", help="create a local .openagent/openagent.env file")
    config_init.add_argument("--workspace", "--dir", dest="workspace", default=None, help="workspace path, default current directory")
    config_init.add_argument("--path", default=None, help="env file path, default <workspace>/.openagent/openagent.env")
    config_init.add_argument("--api-key", default=None, help="optional OpenAI-compatible API key to write")
    config_init.add_argument("--base-url", default=DEFAULT_BASE_URL, help=f"OpenAI-compatible base URL, default {DEFAULT_BASE_URL}")
    config_init.add_argument("--model", default=DEFAULT_MODEL, help=f"model id, default {DEFAULT_MODEL}")
    config_init.add_argument("--wire-api", choices=["chat", "responses"], default=DEFAULT_WIRE_API, help=f"wire API, default {DEFAULT_WIRE_API}")
    config_init.add_argument("--max-steps", default=DEFAULT_MAX_STEPS, help=f"AgentLoop max steps, default {DEFAULT_MAX_STEPS}")
    config_init.add_argument("--with-server-token", action="store_true", help="generate OPENAGENT_SERVER_TOKEN for secured App Bridge access")
    config_init.add_argument("--force", action="store_true", help="overwrite an existing env file")
    config_init.add_argument("--format", choices=["text", "json"], default="text", help="output format")

    config_show = config_subparsers.add_parser("show", help="show resolved local CLI configuration")
    add_common_model_options(config_show)
    config_show.add_argument("--workspace", "--dir", dest="workspace", default=None, help="workspace path, default current directory")
    config_show.add_argument("--session-root", default=None, help="session store root")
    config_show.add_argument("--server-url", default=None, help=f"App Bridge URL, default OPENAGENT_SERVER_URL or {DEFAULT_SERVER_URL}")
    add_server_auth_options(config_show, role="client")
    config_show.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    add_auth_parser(subparsers, "auth", help_text="manage provider credentials")
    add_auth_parser(subparsers, "providers", help_text="manage provider credentials")

    mcp = subparsers.add_parser("mcp", help="manage remote MCP servers")
    mcp_subparsers = mcp.add_subparsers(dest="mcp_command", required=True)

    mcp_list = mcp_subparsers.add_parser("list", aliases=["ls"], help="list configured MCP servers")
    add_mcp_options(mcp_list)
    mcp_list.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    mcp_show = mcp_subparsers.add_parser("show", help="show one configured MCP server")
    add_mcp_options(mcp_show)
    mcp_show.add_argument("name", help="MCP server name")
    mcp_show.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    mcp_add = mcp_subparsers.add_parser("add", help="add or update a remote MCP server")
    add_mcp_options(mcp_add)
    mcp_add.add_argument("name", help="MCP server name")
    mcp_add.add_argument("--url", required=True, help="remote MCP server URL")
    mcp_add.add_argument("--transport", choices=["auto", "http", "sse"], default="auto", help="transport selection")
    mcp_add.add_argument("--header", action="append", default=[], help="HTTP header as KEY=VALUE; can be used more than once")
    mcp_add.add_argument("--timeout-ms", type=int, default=30000, help="request timeout in milliseconds")
    mcp_add.add_argument("--disabled", action="store_true", help="write the server as disabled")
    mcp_add.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    mcp_remove = mcp_subparsers.add_parser("remove", aliases=["rm"], help="remove a configured MCP server")
    add_mcp_options(mcp_remove)
    mcp_remove.add_argument("name", help="MCP server name")
    mcp_remove.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    mcp_doctor = mcp_subparsers.add_parser("doctor", help="validate MCP configuration and optionally refresh remote tools")
    add_mcp_options(mcp_doctor)
    mcp_doctor.add_argument("--refresh", action="store_true", help="refresh remote MCP tool listings")
    mcp_doctor.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    doctor_parser = subparsers.add_parser("doctor", help="check local model gateway configuration")
    add_common_model_options(doctor_parser)
    doctor_parser.add_argument("--format", choices=["text", "json"], default="text", help="output format")
    return parser


def add_common_model_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--config", default=None, help="env file; default .openagent/openagent.env then ~/.openagent/openagent.env")
    parser.add_argument("--auth-file", default=None, help="auth file; default OPENAGENT_AUTH_FILE or ~/.config/openagent/auth.json")
    parser.add_argument("--base-url", default=None, help=f"OpenAI-compatible base URL, default {DEFAULT_BASE_URL}")
    parser.add_argument("--model", default=None, help=f"model id, default {DEFAULT_MODEL}")
    parser.add_argument("--wire-api", choices=["chat", "responses"], default=None, help=f"wire API, default {DEFAULT_WIRE_API}")
    parser.add_argument("--api-key", default=None, help="OpenAI-compatible API key; if omitted, OPENAI_API_KEY is used")
    parser.add_argument("--max-steps", default=None, help=f"AgentLoop max steps, default {DEFAULT_MAX_STEPS}")


def add_tui_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--workspace", default=None, help="workspace path, default current directory")
    parser.add_argument("--session-root", default=None, help="session store root")
    parser.add_argument("--session", "-s", default=None, help="session id to open in the TUI")
    parser.add_argument("--continue", "-c", dest="continue_last", action="store_true", help="open the most recent session")
    parser.add_argument("--skip-doctor", action="store_true", help="start TUI without checking the local model gateway first")


def add_server_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8787)
    parser.add_argument("--workspace", default=None)
    parser.add_argument("--session-root", default=None)


def add_server_auth_options(parser: argparse.ArgumentParser, *, role: str) -> None:
    if role == "client":
        parser.add_argument("--server-token", default=None, help=f"Bearer token for the App Bridge server; default {DEFAULT_SERVER_TOKEN_ENV} env")
        parser.add_argument("--server-token-env", default=DEFAULT_SERVER_TOKEN_ENV, help="environment variable containing the Bearer token")
    else:
        parser.add_argument("--auth-token", default=None, help=f"Bearer token required for API/SSE requests; default {DEFAULT_SERVER_TOKEN_ENV} env")
        parser.add_argument("--auth-token-env", default=DEFAULT_SERVER_TOKEN_ENV, help="environment variable containing the Bearer token")


def add_session_store_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--workspace", "--dir", dest="workspace", default=None, help="workspace path used to resolve the default session root")
    parser.add_argument("--session-root", default=None, help="session store root, default OPENAGENT_SESSION_ROOT or .openagent/sessions")


def add_command_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--workspace", "--dir", dest="workspace", default=None, help="workspace path, default current directory")
    parser.add_argument("--command-dir", action="append", default=[], help="extra custom command directory; can be used more than once")


def add_auth_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--auth-file", default=None, help="auth file; default OPENAGENT_AUTH_FILE or ~/.config/openagent/auth.json")


def add_auth_parser(subparsers: argparse._SubParsersAction, name: str, *, help_text: str) -> None:
    auth = subparsers.add_parser(name, help=help_text)
    auth_subparsers = auth.add_subparsers(dest="auth_command", required=True)

    auth_login = auth_subparsers.add_parser("login", help="store OpenAI-compatible provider credentials")
    add_auth_options(auth_login)
    auth_login.add_argument("--provider", "-p", default="openai", help="provider id, default openai")
    auth_login.add_argument("--type", dest="credential_type", default=None, help="credential type metadata; default api on first login")
    auth_login.add_argument("--api-key", default=None, help="API key to store")
    auth_login.add_argument("--api-key-stdin", action="store_true", help="read API key from stdin")
    auth_login.add_argument("--base-url", default=None, help="OpenAI-compatible base URL")
    auth_login.add_argument("--model", default=None, help="default model id")
    auth_login.add_argument("--wire-api", choices=["chat", "responses"], default=None, help="wire API")

    auth_list = auth_subparsers.add_parser("list", aliases=["ls"], help="list authenticated providers")
    add_auth_options(auth_list)
    auth_list.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    auth_methods = auth_subparsers.add_parser("methods", help="list provider auth methods")
    auth_methods.add_argument("provider", nargs="?", default=None, help="optional provider id")
    auth_methods.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    auth_logout = auth_subparsers.add_parser("logout", help="remove stored provider credentials")
    add_auth_options(auth_logout)
    auth_logout.add_argument("--provider", "-p", default="openai", help="provider id, default openai")


def add_mcp_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--config", dest="mcp_config", default=None, help="MCP JSON config path; default OPENAGENT_MCP_CONFIG or <workspace>/.openagent/mcp.json")
    parser.add_argument("--workspace", "--dir", dest="workspace", default=None, help="workspace path used to resolve the default MCP config")


def apply_model_env(args: argparse.Namespace, *, defaults: OpenAgentCliDefaults = OpenAgentCliDefaults()) -> None:
    set_env_if_value("OPENAI_API_KEY", getattr(args, "api_key", None))
    os.environ["OPENAI_BASE_URL"] = str(getattr(args, "base_url", None) or os.getenv("OPENAI_BASE_URL") or defaults.base_url)
    os.environ["OPENAI_MODEL"] = str(getattr(args, "model", None) or os.getenv("OPENAI_MODEL") or defaults.model)
    os.environ["OPENAI_WIRE_API"] = str(getattr(args, "wire_api", None) or os.getenv("OPENAI_WIRE_API") or defaults.wire_api)
    os.environ["OPENAGENT_APP_MAX_STEPS"] = str(getattr(args, "max_steps", None) or os.getenv("OPENAGENT_APP_MAX_STEPS") or defaults.max_steps)


def run_tui(args: argparse.Namespace) -> None:
    from openagent.tui.app import main as tui_main

    argv: list[str] = []
    workspace = getattr(args, "workspace", None)
    session_root = getattr(args, "session_root", None)
    if workspace:
        argv.extend(["--workspace", str(Path(workspace).expanduser())])
    if session_root:
        argv.extend(["--session-root", str(Path(session_root).expanduser())])
    if getattr(args, "session", None):
        argv.extend(["--session", str(getattr(args, "session"))])
    if getattr(args, "continue_last", False):
        argv.append("--continue")
    tui_main(argv)


def run_attach_command(
    args: argparse.Namespace,
    *,
    tui_main: Callable[..., object] | None = None,
    runtime_factory: object | None = None,
    stderr: object | None = None,
) -> int:
    from openagent.tui.app import main as default_tui_main
    from openagent.tui.remote_runtime import RemoteAppBridgeRuntime

    err = stderr or sys.stderr
    server_url = normalize_server_url(str(getattr(args, "url", None) or ""))
    server_token = resolve_server_token(args, token_attr="server_token", token_env_attr="server_token_env")
    workspace = Path(getattr(args, "workspace", None) or Path.cwd()).expanduser().resolve()
    factory = runtime_factory or RemoteAppBridgeRuntime
    runtime = factory(server_url=server_url, workspace=workspace, auth_token=server_token)  # type: ignore[operator]

    if not getattr(args, "skip_health_check", False):
        try:
            runtime.list_sessions()
        except Exception as error:  # noqa: BLE001 - attach should report connection/auth failures cleanly.
            print(f"OpenAgent attach failed: {error}", file=err)
            return 1

    runner = tui_main or default_tui_main
    runner(
        [],
        runtime=runtime,
        initial_session_id=getattr(args, "session", None),
        continue_last=bool(getattr(args, "continue_last", False)),
    )
    return 0


def run_web(args: argparse.Namespace) -> None:
    run_serve(args)


def run_serve(args: argparse.Namespace, *, serve_fn: Callable[..., object] | None = None) -> None:
    from openagent.app_server.server import serve

    fn = serve_fn or serve
    fn(
        host=str(getattr(args, "host", "127.0.0.1")),
        port=int(getattr(args, "port", 8787)),
        workspace=getattr(args, "workspace", None),
        session_store_root=getattr(args, "session_root", None),
        serve_static=not bool(getattr(args, "headless", False)),
        auth_token=resolve_server_token(args, token_attr="auth_token", token_env_attr="auth_token_env"),
    )


def run_non_interactive(
    args: argparse.Namespace,
    *,
    runtime_factory: object | None = None,
    stdout: object | None = None,
    stderr: object | None = None,
    stdin: object | None = None,
) -> int:
    from openagent.app_server.runtime import OpenAgentAppRuntime

    out = stdout or sys.stdout
    err = stderr or sys.stderr
    source = stdin or sys.stdin
    workspace = Path(getattr(args, "workspace", None) or Path.cwd()).expanduser()
    prompt_text = command_text_from_args(args, stdin=source)
    if getattr(args, "custom_command", None):
        try:
            command = resolve_command(str(args.custom_command), workspace=workspace, extra_dirs=getattr(args, "command_dir", []))
        except FileNotFoundError as error:
            print(str(error), file=err)
            return 1
        if command.model and not getattr(args, "model", None):
            os.environ["OPENAI_MODEL"] = command.model
        prompt_text = render_command(
            command,
            list(getattr(args, "message", []) or []),
            workspace=workspace,
            allow_shell=not bool(getattr(args, "no_command_shell", False)),
        )
    prompt = build_run_prompt(prompt_text, files=getattr(args, "file", []), workspace=workspace)
    if not prompt:
        print("openagent run requires a prompt argument or stdin input.", file=err)
        return 2

    factory = runtime_factory or OpenAgentAppRuntime
    runtime = factory(workspace=workspace, session_store_root=getattr(args, "session_root", None))
    session = select_run_session(runtime, args, workspace=workspace)
    turn = runtime.start_turn(session_id=str(session["id"]), user_text=prompt)
    return emit_turn_events(turn, output_format=str(getattr(args, "format", "text")), verbose=bool(getattr(args, "verbose", False)), stdout=out, stderr=err)


def run_client_command(
    args: argparse.Namespace,
    *,
    stdout: object | None = None,
    stderr: object | None = None,
    stdin: object | None = None,
) -> int:
    out = stdout or sys.stdout
    err = stderr or sys.stderr
    source = stdin or sys.stdin
    server_url = normalize_server_url(str(getattr(args, "server_url", None) or os.getenv("OPENAGENT_SERVER_URL") or DEFAULT_SERVER_URL))
    server_token = resolve_server_token(args, token_attr="server_token", token_env_attr="server_token_env")
    workspace = Path(getattr(args, "workspace", None) or Path.cwd()).expanduser().resolve()
    prompt_text = command_text_from_args(args, stdin=source)
    if getattr(args, "custom_command", None):
        try:
            command = resolve_command(str(args.custom_command), workspace=workspace, extra_dirs=getattr(args, "command_dir", []))
        except FileNotFoundError as error:
            print(str(error), file=err)
            return 1
        prompt_text = render_command(
            command,
            list(getattr(args, "message", []) or []),
            workspace=workspace,
            allow_shell=not bool(getattr(args, "no_command_shell", False)),
        )
    prompt = build_run_prompt(prompt_text, files=getattr(args, "file", []), workspace=workspace)
    if not prompt:
        print("openagent client requires a prompt argument or stdin input.", file=err)
        return 2

    try:
        session = select_client_session(server_url, args, workspace=workspace, auth_token=server_token)
        session_id = str(session.get("id") or "")
        if not session_id:
            raise AppBridgeClientError("server returned a session without an id")
        turn_payload = app_bridge_post_json(server_url, f"/api/sessions/{quote_path(session_id)}/turns", {"input": prompt}, auth_token=server_token)
        turn = turn_payload.get("turn") if isinstance(turn_payload.get("turn"), dict) else {}
        turn_id = str(turn.get("id") or "")
        if not turn_id:
            raise AppBridgeClientError("server returned a turn without an id")
        return emit_app_bridge_events(
            server_url,
            turn_id,
            output_format=str(getattr(args, "format", "text")),
            verbose=bool(getattr(args, "verbose", False)),
            stdout=out,
            stderr=err,
            auth_token=server_token,
        )
    except AppBridgeClientError as error:
        print(f"OpenAgent client failed: {error}", file=err)
        return 1


def command_text_from_args(args: argparse.Namespace, *, stdin: object | None = None) -> str:
    message = " ".join(str(part) for part in getattr(args, "message", [])).strip()
    if message:
        return message
    source = stdin or sys.stdin
    is_tty = getattr(source, "isatty", lambda: True)()
    if is_tty:
        return ""
    return str(source.read()).strip()


def build_run_prompt(message: str, *, files: list[str], workspace: Path) -> str:
    parts = [message.strip()] if message.strip() else []
    for raw_path in files:
        path = Path(raw_path).expanduser()
        if not path.is_absolute():
            path = workspace / path
        content = path.read_text(encoding="utf-8")
        parts.append(f"Attached file: {path}\n\n```text\n{content}\n```")
    return "\n\n".join(parts).strip()


def select_run_session(runtime: object, args: argparse.Namespace, *, workspace: Path) -> dict[str, object]:
    session_id = getattr(args, "session", None)
    if session_id:
        return runtime.resume_session(str(session_id))  # type: ignore[attr-defined,no-any-return]
    if getattr(args, "continue_last", False):
        sessions = runtime.list_sessions()  # type: ignore[attr-defined]
        if sessions:
            return runtime.resume_session(str(sessions[0]["id"]))  # type: ignore[attr-defined,no-any-return]
    return runtime.start_session(cwd=workspace)  # type: ignore[attr-defined,no-any-return]


def select_client_session(server_url: str, args: argparse.Namespace, *, workspace: Path, auth_token: str | None = None) -> dict[str, object]:
    session_id = getattr(args, "session", None)
    if session_id:
        payload = app_bridge_get_json(server_url, f"/api/sessions/{quote_path(str(session_id))}", auth_token=auth_token)
        session = payload.get("session")
        if isinstance(session, dict):
            return session
        raise AppBridgeClientError("server returned an invalid session payload")
    if getattr(args, "continue_last", False):
        payload = app_bridge_get_json(server_url, "/api/sessions", auth_token=auth_token)
        sessions = payload.get("sessions")
        if isinstance(sessions, list) and sessions:
            first = sessions[0]
            if isinstance(first, dict):
                return first
    payload = app_bridge_post_json(server_url, "/api/sessions", {"cwd": str(workspace)}, auth_token=auth_token)
    session = payload.get("session")
    if isinstance(session, dict):
        return session
    raise AppBridgeClientError("server returned an invalid session payload")


def emit_turn_events(
    turn: object,
    *,
    output_format: str,
    verbose: bool,
    stdout: object,
    stderr: object,
) -> int:
    sequence = 1
    printed_answer = False
    while True:
        event = turn.wait_for_sequence(sequence, timeout_s=0.2)  # type: ignore[attr-defined]
        if event is None:
            if getattr(turn, "status", None) in {"completed", "failed", "interrupted"}:
                break
            continue
        sequence += 1
        if output_format == "json":
            print(json.dumps(event.to_dict(), ensure_ascii=False, sort_keys=True), file=stdout)
            continue
        printed_answer = emit_text_event(event, verbose=verbose, stdout=stdout, stderr=stderr) or printed_answer

    status = str(getattr(turn, "status", "failed"))
    final_answer = str(getattr(turn, "final_answer", "") or "")
    if output_format == "text":
        if printed_answer:
            print(file=stdout)
        elif final_answer:
            print(final_answer, file=stdout)
        if status != "completed":
            error = str(getattr(turn, "error", "") or status)
            print(f"OpenAgent run failed: {error}", file=stderr)
    return 0 if status == "completed" else 1


def emit_text_event(event: object, *, verbose: bool, stdout: object, stderr: object) -> bool:
    method = event_method(event)
    params = event_params(event)
    payload = params.get("event") if isinstance(params, dict) else {}
    if method == "item/agentMessage/delta" and isinstance(payload, dict):
        print(str(payload.get("text") or ""), end="", flush=True, file=stdout)
        return True
    if method in {"turn/error", "turn/failed"}:
        error = params.get("error") if isinstance(params, dict) else None
        print(f"{method}: {error or params}", file=stderr)
        return False
    if verbose:
        print(f"[{method}]", file=stderr)
    return False


def emit_app_bridge_events(
    server_url: str,
    turn_id: str,
    *,
    output_format: str,
    verbose: bool,
    stdout: object,
    stderr: object,
    auth_token: str | None = None,
) -> int:
    printed_answer = False
    status = "failed"
    final_answer = ""
    for event in stream_app_bridge_events(server_url, turn_id, auth_token=auth_token):
        if output_format == "json":
            print(json.dumps(event, ensure_ascii=False, sort_keys=True), file=stdout)
        else:
            printed_answer = emit_text_event(event, verbose=verbose, stdout=stdout, stderr=stderr) or printed_answer
        method = event_method(event)
        params = event_params(event)
        if method in {"turn/completed", "turn/failed", "turn/interrupted"}:
            default_status = "completed" if method == "turn/completed" else ("interrupted" if method == "turn/interrupted" else "failed")
            status = str(params.get("status") or default_status)
            final_answer = str(params.get("final_answer") or "")
    if output_format == "text":
        if printed_answer:
            print(file=stdout)
        elif final_answer:
            print(final_answer, file=stdout)
        if status != "completed":
            print(f"OpenAgent client turn failed: {status}", file=stderr)
    return 0 if status == "completed" else 1


def event_method(event: object) -> str:
    if isinstance(event, dict):
        return str(event.get("method") or "")
    return str(getattr(event, "method", ""))


def event_params(event: object) -> dict[str, object]:
    if isinstance(event, dict):
        params = event.get("params")
    else:
        params = getattr(event, "params", {})
    return params if isinstance(params, dict) else {}


class AppBridgeClientError(Exception):
    pass


def app_bridge_get_json(server_url: str, path: str, *, auth_token: str | None = None) -> dict[str, object]:
    return app_bridge_request_json("GET", server_url, path, auth_token=auth_token)


def app_bridge_post_json(server_url: str, path: str, payload: dict[str, object], *, auth_token: str | None = None) -> dict[str, object]:
    return app_bridge_request_json("POST", server_url, path, payload=payload, auth_token=auth_token)


def app_bridge_request_json(
    method: str,
    server_url: str,
    path: str,
    *,
    payload: dict[str, object] | None = None,
    auth_token: str | None = None,
    timeout_s: float = 15.0,
) -> dict[str, object]:
    data = None
    headers = {"Accept": "application/json"}
    if auth_token:
        headers["Authorization"] = f"Bearer {auth_token}"
    if payload is not None:
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url=join_server_url(server_url, path), data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=timeout_s) as response:  # noqa: S310 - user-selected local/remote App Bridge URL.
            raw = response.read().decode("utf-8")
    except urllib.error.HTTPError as error:
        raise AppBridgeClientError(format_http_error(method, path, error)) from error
    except urllib.error.URLError as error:
        raise AppBridgeClientError(str(error.reason)) from error
    value = json.loads(raw or "{}")
    if not isinstance(value, dict):
        raise AppBridgeClientError(f"{method} {path} returned non-object JSON")
    if "error" in value:
        raise AppBridgeClientError(str(value["error"]))
    return value


def stream_app_bridge_events(server_url: str, turn_id: str, *, auth_token: str | None = None) -> Iterator[dict[str, object]]:
    headers = {"Accept": "text/event-stream"}
    if auth_token:
        headers["Authorization"] = f"Bearer {auth_token}"
    request = urllib.request.Request(url=join_server_url(server_url, f"/api/turns/{quote_path(turn_id)}/events"), headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=60) as response:  # noqa: S310 - user-selected local/remote App Bridge URL.
            yield from parse_sse_response(response)
    except urllib.error.HTTPError as error:
        raise AppBridgeClientError(format_http_error("GET", f"/api/turns/{turn_id}/events", error)) from error
    except urllib.error.URLError as error:
        raise AppBridgeClientError(str(error.reason)) from error


def parse_sse_response(response: object) -> Iterator[dict[str, object]]:
    data_lines: list[str] = []
    for raw_line in response:  # type: ignore[operator]
        line = raw_line.decode("utf-8").rstrip("\r\n")
        if not line:
            if data_lines:
                yield parse_sse_data("\n".join(data_lines))
                data_lines = []
            continue
        if line.startswith(":"):
            continue
        if line.startswith("data:"):
            data_lines.append(line.removeprefix("data:").lstrip())
    if data_lines:
        yield parse_sse_data("\n".join(data_lines))


def parse_sse_data(data: str) -> dict[str, object]:
    value = json.loads(data)
    if not isinstance(value, dict):
        raise AppBridgeClientError("SSE event data was not a JSON object")
    return value


def format_http_error(method: str, path: str, error: urllib.error.HTTPError) -> str:
    try:
        raw = error.read().decode("utf-8")
        payload = json.loads(raw)
        if isinstance(payload, dict) and payload.get("error"):
            return f"{method} {path} returned HTTP {error.code}: {payload['error']}"
    except Exception:  # noqa: BLE001 - best-effort error formatting.
        pass
    finally:
        error.close()
    return f"{method} {path} returned HTTP {error.code}"


def normalize_server_url(value: str) -> str:
    return value.rstrip("/")


def join_server_url(server_url: str, path: str) -> str:
    return normalize_server_url(server_url) + "/" + path.lstrip("/")


def quote_path(value: str) -> str:
    return urllib.parse.quote(value, safe="")


def resolve_server_token(args: argparse.Namespace, *, token_attr: str, token_env_attr: str) -> str | None:
    explicit = getattr(args, token_attr, None)
    if explicit:
        return str(explicit)
    env_name = str(getattr(args, token_env_attr, None) or "")
    if not env_name:
        return None
    return os.getenv(env_name) or None


def run_session_command(args: argparse.Namespace, *, stdout: object | None = None, stderr: object | None = None) -> int:
    out = stdout or sys.stdout
    err = stderr or sys.stderr
    root = resolve_session_store_root(args)
    sessions = load_session_records(root)
    command = str(getattr(args, "session_command", ""))
    if command in {"list", "ls"}:
        rows = sessions[: max(0, int(getattr(args, "max_count", 20) or 20))]
        if getattr(args, "format", "table") == "json":
            print(json.dumps({"session_root": str(root), "sessions": rows}, ensure_ascii=False, sort_keys=True), file=out)
        else:
            print_session_table(rows, stdout=out)
        return 0
    if command == "export":
        session_id = str(getattr(args, "session_id", ""))
        try:
            payload = export_session_record(root, session_id)
        except ValueError as error:
            print(str(error), file=err)
            return 2
        except FileNotFoundError:
            print(f"Session not found: {session_id}", file=err)
            return 1
        if getattr(args, "sanitize", False):
            payload = sanitize_export_payload(payload)
        print(json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True), file=out)
        return 0
    if command in {"delete", "rm"}:
        session_id = str(getattr(args, "session_id", ""))
        try:
            session_dir = session_dir_for_id(root, session_id)
        except ValueError as error:
            print(str(error), file=err)
            return 2
        if not session_dir.exists():
            print(f"Session not found: {session_id}", file=err)
            return 1
        shutil.rmtree(session_dir)
        print(json.dumps({"deleted": True, "session_id": session_id, "session_root": str(root)}, ensure_ascii=False, sort_keys=True), file=out)
        return 0
    print(f"Unknown session command: {command}", file=err)
    return 2


def run_models_command(
    args: argparse.Namespace,
    *,
    runtime_factory: object | None = None,
    stdout: object | None = None,
    stderr: object | None = None,
) -> int:
    from openagent.app_server.runtime import OpenAgentAppRuntime

    out = stdout or sys.stdout
    err = stderr or sys.stderr
    factory = runtime_factory or OpenAgentAppRuntime
    runtime = factory(workspace=getattr(args, "workspace", None), session_store_root=getattr(args, "session_root", None))
    try:
        models = runtime.list_models()  # type: ignore[attr-defined]
    except Exception as error:  # noqa: BLE001 - CLI should report provider failures compactly.
        print(f"Could not list models: {error}", file=err)
        return 1
    provider = getattr(args, "provider", None)
    rows = [model for model in models if not provider or str(model.get("provider_id") or "") == str(provider)]
    if getattr(args, "format", "table") == "json":
        print(json.dumps({"models": rows}, ensure_ascii=False, sort_keys=True), file=out)
    else:
        print_model_table(rows, stdout=out)
    return 0


def run_stats_command(args: argparse.Namespace, *, stdout: object | None = None) -> int:
    out = stdout or sys.stdout
    root = resolve_session_store_root(args)
    payload = collect_session_stats(root, days=getattr(args, "days", None))
    if getattr(args, "format", "table") == "json":
        print(json.dumps(payload, ensure_ascii=False, sort_keys=True), file=out)
    else:
        print_stats_table(payload, stdout=out)
    return 0


def run_custom_command(args: argparse.Namespace, *, stdout: object | None = None, stderr: object | None = None) -> int:
    out = stdout or sys.stdout
    err = stderr or sys.stderr
    workspace = Path(getattr(args, "workspace", None) or Path.cwd()).expanduser().resolve()
    action = str(getattr(args, "custom_command_action", ""))
    extra_dirs = list(getattr(args, "command_dir", []) or [])
    if action in {"list", "ls"}:
        commands = [command.to_dict() for command in discover_commands(workspace=workspace, extra_dirs=extra_dirs)]
        if getattr(args, "format", "table") == "json":
            print(json.dumps({"commands": commands}, ensure_ascii=False, sort_keys=True), file=out)
        else:
            print_command_table(commands, stdout=out)
        return 0
    if action == "show":
        try:
            command = resolve_command(str(args.name), workspace=workspace, extra_dirs=extra_dirs)
        except FileNotFoundError as error:
            print(str(error), file=err)
            return 1
        payload = command.to_dict(include_template=True)
        if getattr(args, "format", "text") == "json":
            print(json.dumps(payload, ensure_ascii=False, sort_keys=True), file=out)
        else:
            print_command_detail(payload, stdout=out)
        return 0
    if action == "render":
        try:
            command = resolve_command(str(args.name), workspace=workspace, extra_dirs=extra_dirs)
        except FileNotFoundError as error:
            print(str(error), file=err)
            return 1
        rendered = render_command(command, list(getattr(args, "arguments", []) or []), workspace=workspace, allow_shell=not bool(getattr(args, "no_shell", False)))
        if getattr(args, "format", "text") == "json":
            print(json.dumps({"command": command.to_dict(), "prompt": rendered}, ensure_ascii=False, sort_keys=True), file=out)
        else:
            print(rendered, file=out)
        return 0
    print(f"Unknown command action: {action}", file=err)
    return 2


def run_config_command(args: argparse.Namespace, *, stdout: object | None = None, stderr: object | None = None) -> int:
    out = stdout or sys.stdout
    err = stderr or sys.stderr
    command = str(getattr(args, "config_command", ""))
    if command == "init":
        try:
            payload = init_local_config(args)
        except FileExistsError as error:
            print(str(error), file=err)
            return 1
        if getattr(args, "format", "text") == "json":
            print(json.dumps(payload, ensure_ascii=False, sort_keys=True), file=out)
        else:
            print_config_init_result(payload, stdout=out)
        return 0
    if command == "show":
        workspace = Path(getattr(args, "workspace", None) or Path.cwd()).expanduser().resolve()
        config_path = getattr(args, "config", None)
        if not config_path and (workspace / ".openagent" / "openagent.env").exists():
            config_path = str(workspace / ".openagent" / "openagent.env")
        load_local_env(config_path)
        load_auth_env(getattr(args, "auth_file", None))
        apply_model_env(args)
        payload = collect_config_snapshot(args)
        if getattr(args, "format", "table") == "json":
            print(json.dumps(payload, ensure_ascii=False, sort_keys=True), file=out)
        else:
            print_config_table(payload, stdout=out)
        return 0
    print(f"Unknown config command: {command}", file=err)
    return 2


def init_local_config(args: argparse.Namespace) -> dict[str, object]:
    workspace = Path(getattr(args, "workspace", None) or Path.cwd()).expanduser().resolve()
    path = resolve_config_output_path(args, workspace=workspace)
    if path.exists() and not bool(getattr(args, "force", False)):
        raise FileExistsError(f"Config file already exists: {path}. Re-run with --force to overwrite.")
    server_token = secrets.token_urlsafe(24) if bool(getattr(args, "with_server_token", False)) else None
    lines = [
        "# OpenAgent local configuration",
        "# Keep this file private. It is ignored by git when stored under .openagent/.",
    ]
    api_key = str(getattr(args, "api_key", None) or "")
    lines.append(f"OPENAI_API_KEY={api_key}")
    lines.append(f"OPENAI_BASE_URL={getattr(args, 'base_url', DEFAULT_BASE_URL)}")
    lines.append(f"OPENAI_MODEL={getattr(args, 'model', DEFAULT_MODEL)}")
    lines.append(f"OPENAI_WIRE_API={getattr(args, 'wire_api', DEFAULT_WIRE_API)}")
    lines.append(f"OPENAGENT_APP_MAX_STEPS={getattr(args, 'max_steps', DEFAULT_MAX_STEPS)}")
    if server_token:
        lines.append(f"{DEFAULT_SERVER_TOKEN_ENV}={server_token}")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    os.chmod(path, 0o600)
    return {
        "created": True,
        "path": str(path),
        "workspace": str(workspace),
        "api_key_written": bool(api_key),
        "server_token_written": bool(server_token),
        "mode": oct(path.stat().st_mode & 0o777),
        "next": [
            "openagent doctor",
            "openagent",
        ],
    }


def resolve_config_output_path(args: argparse.Namespace, *, workspace: Path) -> Path:
    raw_path = getattr(args, "path", None) or getattr(args, "config", None)
    if raw_path:
        path = Path(str(raw_path)).expanduser()
        return path if path.is_absolute() else (workspace / path).resolve()
    return workspace / ".openagent" / "openagent.env"


def collect_config_snapshot(args: argparse.Namespace) -> dict[str, object]:
    workspace = Path(getattr(args, "workspace", None) or Path.cwd()).expanduser().resolve()
    session_root = resolve_session_store_root(args)
    auth_file = resolve_auth_file(getattr(args, "auth_file", None))
    env_file = resolve_loaded_env_file(getattr(args, "config", None), workspace=workspace)
    return {
        "workspace": str(workspace),
        "env_file": str(env_file) if env_file else None,
        "auth_file": str(auth_file),
        "session_root": str(session_root),
        "openai": {
            "base_url": os.getenv("OPENAI_BASE_URL") or DEFAULT_BASE_URL,
            "model": os.getenv("OPENAI_MODEL") or DEFAULT_MODEL,
            "wire_api": os.getenv("OPENAI_WIRE_API") or DEFAULT_WIRE_API,
            "api_key": env_secret_state("OPENAI_API_KEY"),
            "max_steps": os.getenv("OPENAGENT_APP_MAX_STEPS") or DEFAULT_MAX_STEPS,
        },
        "app_bridge": {
            "server_url": normalize_server_url(str(getattr(args, "server_url", None) or os.getenv("OPENAGENT_SERVER_URL") or DEFAULT_SERVER_URL)),
            "server_token": "set" if resolve_server_token(args, token_attr="server_token", token_env_attr="server_token_env") else "missing",
            "server_token_env": str(getattr(args, "server_token_env", DEFAULT_SERVER_TOKEN_ENV) or DEFAULT_SERVER_TOKEN_ENV),
        },
    }


def resolve_loaded_env_file(path: str | None, *, workspace: Path | None = None) -> Path | None:
    workspace_path = workspace or Path.cwd()
    candidates = [Path(path).expanduser()] if path else [
        workspace_path / ".openagent" / "openagent.env",
        Path.cwd() / ".openagent" / "openagent.env",
        Path.home() / ".openagent" / "openagent.env",
    ]
    for candidate in candidates:
        if candidate.exists() and candidate.is_file():
            return candidate.resolve()
    return None


def env_secret_state(name: str) -> str:
    return "set" if os.getenv(name) else "missing"


def print_config_init_result(payload: dict[str, object], *, stdout: object) -> None:
    print(f"created: {payload.get('path')}", file=stdout)
    print(f"mode: {payload.get('mode')}", file=stdout)
    print(f"api_key_written: {payload.get('api_key_written')}", file=stdout)
    print(f"server_token_written: {payload.get('server_token_written')}", file=stdout)
    print("next:", file=stdout)
    for item in payload.get("next", []):  # type: ignore[union-attr]
        print(f"  {item}", file=stdout)


def print_config_table(payload: dict[str, object], *, stdout: object) -> None:
    openai = payload.get("openai") if isinstance(payload.get("openai"), dict) else {}
    app_bridge = payload.get("app_bridge") if isinstance(payload.get("app_bridge"), dict) else {}
    rows = [
        ["key", "value"],
        ["workspace", str(payload.get("workspace") or "")],
        ["env_file", str(payload.get("env_file") or "")],
        ["auth_file", str(payload.get("auth_file") or "")],
        ["session_root", str(payload.get("session_root") or "")],
        ["openai.base_url", str(openai.get("base_url") or "")],
        ["openai.model", str(openai.get("model") or "")],
        ["openai.wire_api", str(openai.get("wire_api") or "")],
        ["openai.api_key", str(openai.get("api_key") or "")],
        ["openai.max_steps", str(openai.get("max_steps") or "")],
        ["app_bridge.server_url", str(app_bridge.get("server_url") or "")],
        ["app_bridge.server_token", str(app_bridge.get("server_token") or "")],
        ["app_bridge.server_token_env", str(app_bridge.get("server_token_env") or "")],
    ]
    print_table(rows, stdout=stdout)


def run_auth_command(
    args: argparse.Namespace,
    *,
    stdout: object | None = None,
    stderr: object | None = None,
    stdin: object | None = None,
) -> int:
    out = stdout or sys.stdout
    err = stderr or sys.stderr
    source = stdin or sys.stdin
    command = str(getattr(args, "auth_command", ""))
    auth_file = getattr(args, "auth_file", None)
    if command == "login":
        api_key = getattr(args, "api_key", None)
        if getattr(args, "api_key_stdin", False):
            api_key = str(source.read()).strip()
        try:
            result = login_provider(
                provider=str(getattr(args, "provider", "openai")),
                credential_type=getattr(args, "credential_type", None),
                api_key=api_key,
                base_url=getattr(args, "base_url", None),
                model=getattr(args, "model", None),
                wire_api=getattr(args, "wire_api", None),
                path=auth_file,
            )
        except ValueError as error:
            print(str(error), file=err)
            return 2
        print(json.dumps({"status": "logged_in", **result}, ensure_ascii=False, sort_keys=True), file=out)
        return 0
    if command in {"list", "ls"}:
        providers = list_providers(auth_file)
        if getattr(args, "format", "table") == "json":
            print(json.dumps({"auth_file": str(resolve_auth_file(auth_file)), "providers": providers}, ensure_ascii=False, sort_keys=True), file=out)
        else:
            print_auth_table(providers, stdout=out)
        return 0
    if command == "methods":
        try:
            provider = normalize_provider(getattr(args, "provider", None)) if getattr(args, "provider", None) else None
            rows = provider_method_rows(provider)
        except ValueError as error:
            print(str(error), file=err)
            return 2
        if getattr(args, "format", "table") == "json":
            if provider:
                print(json.dumps({"provider": provider, "methods": rows[0]["methods"] if rows else []}, ensure_ascii=False, sort_keys=True), file=out)
            else:
                print(json.dumps({"providers": rows}, ensure_ascii=False, sort_keys=True), file=out)
        else:
            print_auth_methods_table(rows, stdout=out)
        return 0
    if command == "logout":
        try:
            result = logout_provider(provider=str(getattr(args, "provider", "openai")), path=auth_file)
        except ValueError as error:
            print(str(error), file=err)
            return 2
        print(json.dumps({"status": "logged_out", **result}, ensure_ascii=False, sort_keys=True), file=out)
        return 0
    print(f"Unknown auth command: {command}", file=err)
    return 2


def run_mcp_command(args: argparse.Namespace, *, stdout: object | None = None, stderr: object | None = None) -> int:
    from openagent.cli.mcp import run_mcp_command as run

    return run(args, stdout=stdout, stderr=stderr)


def resolve_session_store_root(args: argparse.Namespace) -> Path:
    workspace = Path(getattr(args, "workspace", None) or Path.cwd()).expanduser().resolve()
    root = Path(str(getattr(args, "session_root", None) or os.getenv("OPENAGENT_SESSION_ROOT") or ".openagent/sessions")).expanduser()
    if not root.is_absolute():
        root = workspace / root
    return root


def load_session_records(root: Path) -> list[dict[str, object]]:
    rows: list[dict[str, object]] = []
    if not root.exists():
        return rows
    for state_path in root.glob("*/state.latest.json"):
        session_id = state_path.parent.name
        state = read_json_file(state_path) or {}
        session_record = read_json_file(state_path.parent / "session.json") or {}
        runs = load_run_records(state_path.parent)
        updated_at = int(state.get("updated_at_ms") or session_record.get("updated_at_ms") or 0)
        rows.append(
            {
                "session_id": str(state.get("session_id") or session_id),
                "workspace": str(state.get("workspace") or session_record.get("workspace") or ""),
                "status": str(state.get("status") or session_record.get("status") or ""),
                "message_count": len([item for item in state.get("messages") or [] if isinstance(item, dict)]),
                "run_count": len(runs),
                "active_run_id": str(session_record.get("active_run_id") or state.get("run_id") or ""),
                "updated_at_ms": updated_at,
                "session_dir": str(state_path.parent),
            }
        )
    return sorted(rows, key=lambda item: int(item.get("updated_at_ms") or 0), reverse=True)


def load_run_records(session_dir: Path) -> list[dict[str, object]]:
    runs: list[dict[str, object]] = []
    for run_json in sorted((session_dir / "runs").glob("*/run.json")):
        run = read_json_file(run_json) or {}
        summary = read_json_file(run_json.parent / "summary.json") or {}
        runs.append(
            {
                "run_id": str(run.get("run_id") or run_json.parent.name),
                "status": str(summary.get("status") or run.get("status") or ""),
                "model_id": run.get("model_id"),
                "provider_id": run.get("provider_id"),
                "started_at_ms": run.get("started_at_ms"),
                "ended_at_ms": run.get("ended_at_ms"),
                "duration_ms": run.get("duration_ms"),
                "message_count": summary.get("message_count", 0),
                "step_count": summary.get("step_count", 0),
                "tool_call_count": summary.get("tool_call_count", 0),
                "runtime_warning_count": summary.get("runtime_warning_count", 0),
                "patch_count": summary.get("patch_count", 0),
                "total_input_tokens": summary.get("total_input_tokens", 0),
                "total_output_tokens": summary.get("total_output_tokens", 0),
                "total_cost": summary.get("total_cost", 0.0),
            }
        )
    return runs


def export_session_record(root: Path, session_id: str) -> dict[str, object]:
    session_dir = session_dir_for_id(root, session_id)
    state = read_json_file(session_dir / "state.latest.json")
    if state is None:
        raise FileNotFoundError(session_id)
    return {
        "schema_version": "openagent.session_export.v1",
        "session_root": str(root),
        "session": state,
        "session_record": read_json_file(session_dir / "session.json") or {},
        "transcript": read_jsonl_file(session_dir / "transcript.jsonl"),
        "runs": load_run_records(session_dir),
    }


def collect_session_stats(root: Path, *, days: int | None = None) -> dict[str, object]:
    sessions = load_session_records(root)
    min_updated_at = None
    if days is not None and days > 0:
        import time

        min_updated_at = int((time.time() - days * 86400) * 1000)
        sessions = [session for session in sessions if int(session.get("updated_at_ms") or 0) >= min_updated_at]

    runs: list[dict[str, object]] = []
    for session in sessions:
        session_dir = Path(str(session.get("session_dir") or ""))
        for run in load_run_records(session_dir):
            run["session_id"] = session.get("session_id")
            runs.append(run)

    return {
        "session_root": str(root),
        "days": days,
        "since_updated_at_ms": min_updated_at,
        "session_count": len(sessions),
        "run_count": len(runs),
        "message_count": sum(int(session.get("message_count") or 0) for session in sessions),
        "step_count": sum(int(run.get("step_count") or 0) for run in runs),
        "tool_call_count": sum(int(run.get("tool_call_count") or 0) for run in runs),
        "runtime_warning_count": sum(int(run.get("runtime_warning_count") or 0) for run in runs),
        "patch_count": sum(int(run.get("patch_count") or 0) for run in runs),
        "total_input_tokens": sum(int(run.get("total_input_tokens") or 0) for run in runs),
        "total_output_tokens": sum(int(run.get("total_output_tokens") or 0) for run in runs),
        "total_cost": sum(float(run.get("total_cost") or 0.0) for run in runs),
        "model_counts": count_values(str(run.get("model_id") or "") for run in runs),
    }


def print_session_table(rows: list[dict[str, object]], *, stdout: object) -> None:
    if not rows:
        print("No sessions found.", file=stdout)
        return
    table = [["updated_ms", "session", "status", "msgs", "runs", "workspace"]]
    for row in rows:
        table.append(
            [
                str(row.get("updated_at_ms") or ""),
                str(row.get("session_id") or ""),
                str(row.get("status") or ""),
                str(row.get("message_count") or 0),
                str(row.get("run_count") or 0),
                str(row.get("workspace") or ""),
            ]
        )
    print_table(table, stdout=stdout)


def print_model_table(rows: list[dict[str, object]], *, stdout: object) -> None:
    if not rows:
        print("No models found.", file=stdout)
        return
    table = [["provider", "model", "context", "max_output"]]
    for row in rows:
        table.append(
            [
                str(row.get("provider_id") or ""),
                str(row.get("id") or ""),
                str(row.get("context_window") or ""),
                str(row.get("max_output") or ""),
            ]
        )
    print_table(table, stdout=stdout)


def print_stats_table(payload: dict[str, object], *, stdout: object) -> None:
    rows = [
        ["metric", "value"],
        ["sessions", str(payload.get("session_count") or 0)],
        ["runs", str(payload.get("run_count") or 0)],
        ["messages", str(payload.get("message_count") or 0)],
        ["steps", str(payload.get("step_count") or 0)],
        ["tool_calls", str(payload.get("tool_call_count") or 0)],
        ["runtime_warnings", str(payload.get("runtime_warning_count") or 0)],
        ["patches", str(payload.get("patch_count") or 0)],
        ["input_tokens", str(payload.get("total_input_tokens") or 0)],
        ["output_tokens", str(payload.get("total_output_tokens") or 0)],
        ["cost", f"{float(payload.get('total_cost') or 0.0):.6f}"],
    ]
    print_table(rows, stdout=stdout)


def print_command_table(rows: list[dict[str, object]], *, stdout: object) -> None:
    if not rows:
        print("No custom commands found.", file=stdout)
        return
    table = [["name", "scope", "description", "model"]]
    for row in rows:
        table.append(
            [
                str(row.get("name") or ""),
                str(row.get("scope") or ""),
                str(row.get("description") or ""),
                str(row.get("model") or ""),
            ]
        )
    print_table(table, stdout=stdout)


def print_command_detail(payload: dict[str, object], *, stdout: object) -> None:
    print(f"name: {payload.get('name') or ''}", file=stdout)
    print(f"scope: {payload.get('scope') or ''}", file=stdout)
    print(f"path: {payload.get('path') or ''}", file=stdout)
    if payload.get("description"):
        print(f"description: {payload.get('description')}", file=stdout)
    if payload.get("agent"):
        print(f"agent: {payload.get('agent')}", file=stdout)
    if payload.get("model"):
        print(f"model: {payload.get('model')}", file=stdout)
    print("", file=stdout)
    print(str(payload.get("template") or ""), file=stdout)


def print_auth_table(rows: list[dict[str, object]], *, stdout: object) -> None:
    if not rows:
        print("No authenticated providers found.", file=stdout)
        return
    table = [["provider", "type", "api_key", "env_api_key", "base_url", "model", "wire_api"]]
    for row in rows:
        env_status = row.get("env_status") if isinstance(row.get("env_status"), dict) else {}
        api_key_env = env_status.get("api_key") if isinstance(env_status.get("api_key"), dict) else {}
        table.append(
            [
                str(row.get("provider") or ""),
                str(row.get("type") or ""),
                str(row.get("api_key") or ""),
                str(api_key_env.get("status") or ""),
                str(row.get("base_url") or ""),
                str(row.get("model") or ""),
                str(row.get("wire_api") or ""),
            ]
        )
    print_table(table, stdout=stdout)


def provider_method_rows(provider: str | None = None) -> list[dict[str, object]]:
    provider_ids = [provider] if provider else known_provider_ids()
    return [{"provider": provider_id, "methods": provider_auth_methods(provider_id)} for provider_id in provider_ids]


def print_auth_methods_table(rows: list[dict[str, object]], *, stdout: object) -> None:
    if not rows:
        print("No provider auth methods found.", file=stdout)
        return
    table = [["provider", "method", "type", "env_api_key", "status", "default_base_url"]]
    for row in rows:
        provider = str(row.get("provider") or "")
        methods = row.get("methods") if isinstance(row.get("methods"), list) else []
        for method in methods:
            if not isinstance(method, dict):
                continue
            env = method.get("env") if isinstance(method.get("env"), dict) else {}
            table.append(
                [
                    provider,
                    str(method.get("id") or ""),
                    str(method.get("type") or ""),
                    str(env.get("api_key") or ""),
                    str(method.get("status") or ""),
                    str(method.get("default_base_url") or ""),
                ]
            )
    print_table(table, stdout=stdout)


def print_table(rows: list[list[str]], *, stdout: object) -> None:
    widths = [max(len(row[index]) for row in rows) for index in range(len(rows[0]))]
    for row_index, row in enumerate(rows):
        line = "  ".join(value.ljust(widths[index]) for index, value in enumerate(row)).rstrip()
        print(line, file=stdout)
        if row_index == 0:
            print("  ".join("-" * width for width in widths).rstrip(), file=stdout)


def count_values(values: object) -> dict[str, int]:
    counts: dict[str, int] = {}
    for value in values:  # type: ignore[union-attr]
        if not value:
            continue
        counts[str(value)] = counts.get(str(value), 0) + 1
    return dict(sorted(counts.items()))


def session_dir_for_id(root: Path, session_id: str) -> Path:
    if not session_id or Path(session_id).name != session_id or session_id in {".", ".."}:
        raise ValueError(f"Invalid session id: {session_id}")
    session_dir = (root / session_id).resolve()
    root_dir = root.resolve()
    if session_dir != root_dir and root_dir in session_dir.parents:
        return session_dir
    raise ValueError(f"Invalid session id: {session_id}")


def sanitize_export_payload(value: object) -> object:
    if isinstance(value, dict):
        sanitized: dict[str, object] = {}
        for key, item in value.items():
            lowered = str(key).lower()
            if lowered in {"content", "workspace", "session_root", "session_dir", "transcript_path", "state_path", "ledger_path", "run_dir", "parts_path"}:
                sanitized[str(key)] = "[redacted]"
            elif lowered.endswith("_path") or lowered.endswith("_dir"):
                sanitized[str(key)] = "[redacted]"
            else:
                sanitized[str(key)] = sanitize_export_payload(item)
        return sanitized
    if isinstance(value, list):
        return [sanitize_export_payload(item) for item in value]
    return value


def read_json_file(path: Path) -> dict[str, object] | None:
    if not path.exists():
        return None
    payload = json.loads(path.read_text(encoding="utf-8"))
    return payload if isinstance(payload, dict) else None


def read_jsonl_file(path: Path) -> list[dict[str, object]]:
    if not path.exists():
        return []
    rows: list[dict[str, object]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        payload = json.loads(line)
        if isinstance(payload, dict):
            rows.append(payload)
    return rows


def check_models_endpoint(*, base_url: str, timeout_s: float = 3.0) -> tuple[bool, str]:
    api_key = os.getenv("OPENAI_API_KEY") or ""
    for url in candidate_model_urls(base_url):
        request = urllib.request.Request(url=url, method="GET")
        if api_key:
            request.add_header("Authorization", f"Bearer {api_key}")
        try:
            with urllib.request.urlopen(request, timeout=timeout_s) as response:
                status = int(getattr(response, "status", 0) or 0)
                if 200 <= status < 300:
                    return True, url
                return False, f"{url} returned HTTP {status}"
        except urllib.error.HTTPError as error:
            if error.code in {401, 403}:
                return False, f"{url} returned HTTP {error.code}; check OPENAI_API_KEY"
            if error.code == 404:
                continue
            return False, f"{url} returned HTTP {error.code}"
        except urllib.error.URLError as error:
            last_message = str(error.reason)
        except TimeoutError:
            last_message = "timeout"
    return False, last_message if "last_message" in locals() else "no candidate endpoint responded"


def candidate_model_urls(base_url: str) -> list[str]:
    base = base_url.rstrip("/")
    if base.endswith("/v1"):
        return [f"{base}/models"]
    return [f"{base}/v1/models", f"{base}/models"]


def set_env_if_value(name: str, value: str | None) -> None:
    if value:
        os.environ[name] = value


def load_local_env(path: str | None) -> Path | None:
    candidates = [Path(path).expanduser()] if path else [
        Path.cwd() / ".openagent" / "openagent.env",
        Path.home() / ".openagent" / "openagent.env",
    ]
    for candidate in candidates:
        if not candidate.exists() or not candidate.is_file():
            continue
        for raw_line in candidate.read_text(encoding="utf-8").splitlines():
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("export "):
                line = line.removeprefix("export ").strip()
            if "=" not in line:
                continue
            key, value = line.split("=", maxsplit=1)
            key = key.strip()
            value = unquote_env_value(value.strip())
            if key and key not in os.environ:
                os.environ[key] = value
        return candidate
    return None


def unquote_env_value(value: str) -> str:
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value


if __name__ == "__main__":
    main()
