from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path

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
    run.add_argument("--format", choices=["text", "json"], default="text", help="output format")
    run.add_argument("--verbose", action="store_true", help="show non-answer runtime events in text mode")
    run.add_argument("--skip-doctor", action="store_true", help="run without checking the local model gateway first")

    doctor_parser = subparsers.add_parser("doctor", help="check local model gateway configuration")
    add_common_model_options(doctor_parser)
    return parser


def add_common_model_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--config", default=None, help="env file; default .openagent/openagent.env then ~/.openagent/openagent.env")
    parser.add_argument("--base-url", default=None, help=f"OpenAI-compatible base URL, default {DEFAULT_BASE_URL}")
    parser.add_argument("--model", default=None, help=f"model id, default {DEFAULT_MODEL}")
    parser.add_argument("--wire-api", choices=["chat", "responses"], default=None, help=f"wire API, default {DEFAULT_WIRE_API}")
    parser.add_argument("--api-key", default=None, help="OpenAI-compatible API key; if omitted, OPENAI_API_KEY is used")
    parser.add_argument("--max-steps", default=None, help=f"AgentLoop max steps, default {DEFAULT_MAX_STEPS}")


def add_tui_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--workspace", default=None, help="workspace path, default current directory")
    parser.add_argument("--session-root", default=None, help="session store root")
    parser.add_argument("--skip-doctor", action="store_true", help="start TUI without checking the local model gateway first")


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
    prompt = build_run_prompt(command_text_from_args(args, stdin=source), files=getattr(args, "file", []), workspace=workspace)
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
