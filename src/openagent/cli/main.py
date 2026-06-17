from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path

from openagent.cli.auth import list_providers, load_auth_env, login_provider, logout_provider, resolve_auth_file
from openagent.cli.custom_commands import discover_commands, render_command, resolve_command

DEFAULT_BASE_URL = "http://localhost:8080"
DEFAULT_MODEL = "gpt-5.5"
DEFAULT_WIRE_API = "responses"
DEFAULT_MAX_STEPS = "30"


@dataclass(frozen=True, slots=True)
class OpenAgentCliDefaults:
    base_url: str = DEFAULT_BASE_URL
    model: str = DEFAULT_MODEL
    wire_api: str = DEFAULT_WIRE_API
    max_steps: str = DEFAULT_MAX_STEPS


def main(argv: list[str] | None = None) -> None:
    parser = build_parser()
    args = parser.parse_args(argv)
    command = args.command or "tui"
    load_local_env(getattr(args, "config", None))
    load_auth_env(getattr(args, "auth_file", None))

    if command == "doctor":
        apply_model_env(args)
        ok = doctor(verbose=True)
        raise SystemExit(0 if ok else 2)
    if command == "web":
        apply_model_env(args)
        run_web(args)
        return
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
    if command == "auth":
        raise SystemExit(run_auth_command(args))
    if command == "tui":
        apply_model_env(args)
        if not args.skip_doctor and not doctor(verbose=True):
            print("\nGateway check failed. Start your local OpenAI-compatible service, or rerun with --skip-doctor.", file=sys.stderr)
            raise SystemExit(2)
        run_tui(args)
        return

    parser.error(f"unknown command: {command}")


def doctor(*, verbose: bool = False) -> bool:
    base_url = os.getenv("OPENAI_BASE_URL") or DEFAULT_BASE_URL
    model = os.getenv("OPENAI_MODEL") or DEFAULT_MODEL
    wire_api = os.getenv("OPENAI_WIRE_API") or DEFAULT_WIRE_API
    api_key_set = bool(os.getenv("OPENAI_API_KEY"))
    models_ok, models_message = check_models_endpoint(base_url=base_url)

    if verbose:
        print("OpenAgent doctor")
        print(f"- OPENAI_BASE_URL: {base_url}")
        print(f"- OPENAI_MODEL: {model}")
        print(f"- OPENAI_WIRE_API: {wire_api}")
        print(f"- OPENAI_API_KEY: {'set' if api_key_set else 'missing'}")
        print(f"- model endpoint: {'ok' if models_ok else 'failed'} ({models_message})")
    return models_ok


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

    web = subparsers.add_parser("web", help="start the browser console")
    add_common_model_options(web)
    web.add_argument("--host", default="127.0.0.1")
    web.add_argument("--port", type=int, default=8787)
    web.add_argument("--workspace", default=None)

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

    auth = subparsers.add_parser("auth", help="manage provider credentials")
    auth_subparsers = auth.add_subparsers(dest="auth_command", required=True)

    auth_login = auth_subparsers.add_parser("login", help="store OpenAI-compatible provider credentials")
    add_auth_options(auth_login)
    auth_login.add_argument("--provider", "-p", default="openai", help="provider id, currently openai")
    auth_login.add_argument("--api-key", default=None, help="API key to store")
    auth_login.add_argument("--api-key-stdin", action="store_true", help="read API key from stdin")
    auth_login.add_argument("--base-url", default=None, help="OpenAI-compatible base URL")
    auth_login.add_argument("--model", default=None, help="default model id")
    auth_login.add_argument("--wire-api", choices=["chat", "responses"], default=None, help="wire API")

    auth_list = auth_subparsers.add_parser("list", aliases=["ls"], help="list authenticated providers")
    add_auth_options(auth_list)
    auth_list.add_argument("--format", choices=["table", "json"], default="table", help="output format")

    auth_logout = auth_subparsers.add_parser("logout", help="remove stored provider credentials")
    add_auth_options(auth_logout)
    auth_logout.add_argument("--provider", "-p", default="openai", help="provider id, currently openai")

    doctor_parser = subparsers.add_parser("doctor", help="check local model gateway configuration")
    add_common_model_options(doctor_parser)
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
    parser.add_argument("--skip-doctor", action="store_true", help="start TUI without checking the local model gateway first")


def add_session_store_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--workspace", "--dir", dest="workspace", default=None, help="workspace path used to resolve the default session root")
    parser.add_argument("--session-root", default=None, help="session store root, default OPENAGENT_SESSION_ROOT or .openagent/sessions")


def add_command_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--workspace", "--dir", dest="workspace", default=None, help="workspace path, default current directory")
    parser.add_argument("--command-dir", action="append", default=[], help="extra custom command directory; can be used more than once")


def add_auth_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--auth-file", default=None, help="auth file; default OPENAGENT_AUTH_FILE or ~/.config/openagent/auth.json")


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
    tui_main(argv)


def run_web(args: argparse.Namespace) -> None:
    from openagent.app_server.server import serve

    serve(
        host=str(getattr(args, "host", "127.0.0.1")),
        port=int(getattr(args, "port", 8787)),
        workspace=getattr(args, "workspace", None),
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
    method = str(getattr(event, "method", ""))
    params = getattr(event, "params", {}) or {}
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
    table = [["provider", "api_key", "base_url", "model", "wire_api"]]
    for row in rows:
        table.append(
            [
                str(row.get("provider") or ""),
                str(row.get("api_key") or ""),
                str(row.get("base_url") or ""),
                str(row.get("model") or ""),
                str(row.get("wire_api") or ""),
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
