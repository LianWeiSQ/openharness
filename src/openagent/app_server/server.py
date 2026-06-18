from __future__ import annotations

import argparse
import hmac
import json
import mimetypes
import os
import sys
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

from .runtime import OpenAgentAppRuntime

STATIC_DIR = Path(__file__).resolve().parent / "static"


class OpenAgentThreadingHTTPServer(ThreadingHTTPServer):
    def handle_error(self, request: object, client_address: object) -> None:
        error = sys.exc_info()[1]
        if isinstance(error, (BrokenPipeError, ConnectionResetError, TimeoutError)):
            return
        super().handle_error(request, client_address)


class OpenAgentAppRequestHandler(BaseHTTPRequestHandler):
    runtime: OpenAgentAppRuntime
    serve_static: bool = True
    auth_token: str | None = None

    server_version = "OpenAgentAppServer/0.1"

    def do_OPTIONS(self) -> None:  # noqa: N802
        self.send_response(HTTPStatus.NO_CONTENT)
        self._common_headers()
        self.end_headers()

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        path = parsed.path
        if _is_authenticated_app_path(path) and not self._authorize_api_request():
            return
        try:
            if path == "/api/health":
                self._send_json({"ok": True, "service": "openagent-app-server", "ui_enabled": self.serve_static, "auth_required": bool(self.auth_token)})
            elif path == "/tui/control/next":
                query = parse_qs(parsed.query)
                timeout_s = min(5.0, max(0.0, _query_float(query, "timeout", 0.25)))
                request = self.runtime.wait_for_tui_control(timeout_s=timeout_s)
                self._send_json(request.to_dict() if request is not None else {"path": "", "body": None})
            elif path == "/api/models":
                self._send_json({"models": self.runtime.list_models()})
            elif path == "/api/sessions":
                self._send_json({"sessions": self.runtime.list_sessions()})
            elif path == "/api/events":
                query = parse_qs(parsed.query)
                header_sequence = _header_int(self.headers.get("Last-Event-ID"), 0)
                last_sequence = _query_int(query, "last_sequence", header_sequence)
                self._stream_global_events(last_sequence=last_sequence)
            elif path.startswith("/api/sessions/"):
                session_id = path.removeprefix("/api/sessions/").strip("/")
                if not session_id:
                    self._send_error(HTTPStatus.NOT_FOUND, "session id is required")
                else:
                    self._send_json({"session": self.runtime.get_session(session_id)})
            elif path.startswith("/api/turns/") and path.endswith("/events"):
                turn_id = path.removeprefix("/api/turns/").removesuffix("/events").strip("/")
                query = parse_qs(parsed.query)
                header_sequence = _header_int(self.headers.get("Last-Event-ID"), 0)
                last_sequence = _query_int(query, "last_sequence", header_sequence)
                self._stream_turn_events(turn_id, last_sequence=last_sequence)
            elif not self.serve_static:
                self._send_error(HTTPStatus.NOT_FOUND, "unknown endpoint")
            else:
                self._serve_static(path)
        except (FileNotFoundError, KeyError) as error:
            self._send_error(HTTPStatus.NOT_FOUND, str(error))
        except Exception as error:  # noqa: BLE001
            self._send_error(HTTPStatus.INTERNAL_SERVER_ERROR, str(error))

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        path = parsed.path
        if _is_authenticated_app_path(path) and not self._authorize_api_request():
            return
        try:
            if path == "/tui/control/response":
                response = self.runtime.record_tui_control_response(self._read_json_value())
                self._send_json({"ok": True, "response": response})
                return
            payload = self._read_json()
            if path.startswith("/tui/"):
                self._handle_tui_post(path, payload)
            elif path == "/api/sessions":
                session = self.runtime.start_session(cwd=payload.get("cwd"))
                self._send_json({"session": session}, status=HTTPStatus.CREATED)
            elif path.startswith("/api/sessions/") and path.endswith("/turns"):
                session_id = path.removeprefix("/api/sessions/").removesuffix("/turns").strip("/")
                user_text = str(payload.get("input") or payload.get("user_text") or "")
                turn = _start_runtime_turn(
                    self.runtime,
                    session_id=session_id,
                    user_text=user_text,
                    model_id=_optional_string(payload, "model_id"),
                    provider_id=_optional_string(payload, "provider_id"),
                    agent_name=_optional_string(payload, "agent_name"),
                    variant=_optional_string(payload, "variant"),
                )
                self._send_json({"turn": turn.to_dict()}, status=HTTPStatus.CREATED)
            elif path.startswith("/api/turns/") and path.endswith("/interrupt"):
                turn_id = path.removeprefix("/api/turns/").removesuffix("/interrupt").strip("/")
                turn = self.runtime.interrupt_turn(turn_id)
                self._send_json({"turn": turn})
            elif path.startswith("/api/turns/") and "/approvals/" in path:
                turn_id, request_id = _parse_turn_approval_path(path)
                event = self.runtime.respond_approval(turn_id, request_id, str(payload.get("action") or ""))
                self._send_json({"event": event})
            else:
                self._send_error(HTTPStatus.NOT_FOUND, "unknown endpoint")
        except ValueError as error:
            self._send_error(HTTPStatus.BAD_REQUEST, str(error))
        except (FileNotFoundError, KeyError) as error:
            self._send_error(HTTPStatus.NOT_FOUND, str(error))
        except Exception as error:  # noqa: BLE001
            self._send_error(HTTPStatus.INTERNAL_SERVER_ERROR, str(error))

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
        return

    def _authorize_api_request(self) -> bool:
        if not self.auth_token:
            return True
        expected = f"Bearer {self.auth_token}"
        actual = self.headers.get("Authorization") or ""
        if hmac.compare_digest(actual, expected):
            return True
        self.send_response(HTTPStatus.UNAUTHORIZED)
        self._common_headers(content_type="application/json; charset=utf-8")
        self.send_header("WWW-Authenticate", 'Bearer realm="openagent-app-bridge"')
        self.end_headers()
        self.wfile.write(json.dumps({"error": "unauthorized"}, ensure_ascii=False).encode("utf-8"))
        return False

    def _stream_turn_events(self, turn_id: str, *, last_sequence: int) -> None:
        turn = self.runtime.get_turn(turn_id)
        self.send_response(HTTPStatus.OK)
        self._common_headers(content_type="text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.end_headers()

        sequence = max(1, last_sequence + 1)
        try:
            while True:
                event = turn.wait_for_sequence(sequence, timeout_s=10.0)
                if event is None:
                    self._write_sse_comment("ping")
                    if turn.status in {"completed", "failed", "interrupted"} and sequence > len(turn.events):
                        break
                    continue
                self._write_sse(event.method, event.to_dict(), event_id=str(event.sequence))
                sequence = event.sequence + 1
                if event.method in {"turn/completed", "turn/failed", "turn/interrupted"}:
                    break
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            return

    def _stream_global_events(self, *, last_sequence: int) -> None:
        self.send_response(HTTPStatus.OK)
        self._common_headers(content_type="text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.end_headers()

        sequence = max(1, last_sequence + 1)
        try:
            while True:
                event = self.runtime.wait_for_global_sequence(sequence, timeout_s=10.0)
                if event is None:
                    self._write_sse_comment("ping")
                    continue
                global_sequence = event.global_sequence or sequence
                self._write_sse(event.method, event.to_dict(), event_id=str(global_sequence))
                sequence = global_sequence + 1
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            return

    def _handle_tui_post(self, path: str, payload: dict[str, Any]) -> None:
        if path == "/tui/append-prompt":
            text = _required_string(payload, "text")
            self._send_control_enqueued(path, {"text": text})
            return
        if path == "/tui/submit-prompt":
            self._send_control_enqueued(path, {})
            return
        if path == "/tui/clear-prompt":
            self._send_control_enqueued(path, {})
            return
        if path == "/tui/open-help":
            self._send_control_enqueued(path, {})
            return
        if path == "/tui/open-sessions":
            self._send_control_enqueued(path, {})
            return
        if path == "/tui/open-themes":
            self._send_control_enqueued(path, {})
            return
        if path == "/tui/open-models":
            self._send_control_enqueued(path, {})
            return
        if path == "/tui/open-agents":
            self._send_control_enqueued(path, {})
            return
        if path == "/tui/open-variants":
            self._send_control_enqueued(path, {})
            return
        if path == "/tui/select-model":
            params = {"modelID": _required_string(payload, "modelID")}
            if "providerID" in payload and payload["providerID"] is not None:
                params["providerID"] = _required_string(payload, "providerID")
            self._send_control_enqueued(path, params)
            return
        if path == "/tui/select-agent":
            self._send_control_enqueued(path, {"agent": _required_string(payload, "agent")})
            return
        if path == "/tui/select-variant":
            self._send_control_enqueued(path, {"variant": _required_string(payload, "variant")})
            return
        if path == "/tui/execute-command":
            command = _required_string(payload, "command")
            self._send_control_enqueued(path, {"command": command})
            return
        if path == "/tui/show-toast":
            message = _required_string(payload, "message")
            params: dict[str, Any] = {"message": message}
            for key in ("title", "variant"):
                if key in payload and payload[key] is not None:
                    if not isinstance(payload[key], str):
                        raise ValueError(f"{key} must be a string")
                    params[key] = payload[key]
            if "duration" in payload and payload["duration"] is not None:
                if not isinstance(payload["duration"], int | float):
                    raise ValueError("duration must be a number")
                params["duration"] = payload["duration"]
            self._send_control_enqueued(path, params)
            return
        if path == "/tui/select-session":
            session_id = _required_string(payload, "sessionID")
            self._verify_tui_session_exists(session_id)
            self._send_control_enqueued(path, {"sessionID": session_id})
            return
        if path == "/tui/publish":
            action, params = _publish_to_control(payload)
            if action == "session.select":
                self._verify_tui_session_exists(str(params.get("sessionID") or ""))
            self._send_control_enqueued(path, payload)
            return
        self._send_error(HTTPStatus.NOT_FOUND, "unknown endpoint")

    def _send_control_enqueued(self, path: str, body: Any) -> None:
        request = self.runtime.enqueue_tui_control(path, body)
        self._send_json({"ok": True, "request": request.to_dict()})

    def _verify_tui_session_exists(self, session_id: str) -> None:
        if not session_id:
            raise ValueError("sessionID is required")
        get_session = getattr(self.runtime, "get_session", None)
        if callable(get_session):
            get_session(session_id)
            return
        list_sessions = getattr(self.runtime, "list_sessions", None)
        if callable(list_sessions):
            sessions = list_sessions()
            if isinstance(sessions, list) and not any(str(item.get("id") or "") == session_id for item in sessions if isinstance(item, dict)):
                raise KeyError(f"Unknown session: {session_id}")

    def _serve_static(self, path: str) -> None:
        target = "index.html" if path in {"", "/"} else path.lstrip("/")
        static_path = (STATIC_DIR / target).resolve()
        if not str(static_path).startswith(str(STATIC_DIR.resolve())) or not static_path.exists() or not static_path.is_file():
            static_path = STATIC_DIR / "index.html"
        content_type = mimetypes.guess_type(str(static_path))[0] or "application/octet-stream"
        self.send_response(HTTPStatus.OK)
        self._common_headers(content_type=content_type)
        self.end_headers()
        self.wfile.write(static_path.read_bytes())

    def _read_json(self) -> dict[str, Any]:
        value = self._read_json_value()
        if not isinstance(value, dict):
            raise ValueError("JSON object body is required")
        return value

    def _read_json_value(self) -> Any:
        raw_len = int(self.headers.get("Content-Length") or "0")
        if raw_len <= 0:
            return {}
        raw = self.rfile.read(raw_len).decode("utf-8")
        return json.loads(raw)

    def _send_json(self, payload: dict[str, Any], *, status: HTTPStatus = HTTPStatus.OK) -> None:
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self._common_headers(content_type="application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _send_error(self, status: HTTPStatus, message: str) -> None:
        self._send_json({"error": message}, status=status)

    def _common_headers(self, *, content_type: str | None = None) -> None:
        if content_type:
            self.send_header("Content-Type", content_type)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Authorization, Content-Type, Last-Event-ID")

    def _write_sse(self, event: str, payload: dict[str, Any], *, event_id: str) -> None:
        data = json.dumps(payload, ensure_ascii=False)
        self.wfile.write(f"id: {event_id}\n".encode("utf-8"))
        self.wfile.write(f"event: {event}\n".encode("utf-8"))
        self.wfile.write(f"data: {data}\n\n".encode("utf-8"))
        self.wfile.flush()

    def _write_sse_comment(self, value: str) -> None:
        self.wfile.write(f": {value}\n\n".encode("utf-8"))
        self.wfile.flush()


def create_server(
    *,
    host: str,
    port: int,
    workspace: str | Path | None = None,
    session_store_root: str | Path | None = None,
    serve_static: bool = True,
    auth_token: str | None = None,
    runtime: OpenAgentAppRuntime | None = None,
) -> ThreadingHTTPServer:
    runtime = runtime or OpenAgentAppRuntime(workspace=workspace, session_store_root=session_store_root)

    class Handler(OpenAgentAppRequestHandler):
        pass

    Handler.runtime = runtime
    Handler.serve_static = serve_static
    Handler.auth_token = auth_token or None
    return OpenAgentThreadingHTTPServer((host, port), Handler)


def serve(
    *,
    host: str,
    port: int,
    workspace: str | Path | None = None,
    session_store_root: str | Path | None = None,
    serve_static: bool = True,
    auth_token: str | None = None,
) -> ThreadingHTTPServer:
    httpd = create_server(host=host, port=port, workspace=workspace, session_store_root=session_store_root, serve_static=serve_static, auth_token=auth_token)
    mode = "console" if serve_static else "headless"
    auth_state = "secured" if auth_token else "unsecured"
    print(f"OpenAgent app bridge ({mode}, {auth_state}) listening on http://{host}:{port}")  # noqa: T201
    httpd.serve_forever()
    return httpd


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description="Run the local OpenAgent App Bridge UI.")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8787)
    parser.add_argument("--workspace", default=None)
    parser.add_argument("--session-root", default=None)
    parser.add_argument("--headless", action="store_true", help="serve API/SSE endpoints without the static console")
    parser.add_argument("--auth-token", default=None, help="Bearer token required for API/SSE requests")
    parser.add_argument("--auth-token-env", default="OPENAGENT_SERVER_TOKEN", help="environment variable containing the Bearer token")
    args = parser.parse_args(argv)
    serve(
        host=args.host,
        port=args.port,
        workspace=args.workspace,
        session_store_root=args.session_root,
        serve_static=not args.headless,
        auth_token=args.auth_token or os.getenv(str(args.auth_token_env or "")) or None,
    )


def _is_authenticated_app_path(path: str) -> bool:
    return path.startswith("/api/") or path.startswith("/tui/")


def _parse_turn_approval_path(path: str) -> tuple[str, str]:
    raw = path.removeprefix("/api/turns/")
    turn_id, marker, request_id = raw.partition("/approvals/")
    if not turn_id or marker != "/approvals/" or not request_id.strip("/"):
        raise ValueError("approval path must be /api/turns/{turn_id}/approvals/{request_id}")
    return turn_id.strip("/"), request_id.strip("/")


def _query_int(query: dict[str, list[str]], key: str, default: int) -> int:
    try:
        return int((query.get(key) or [default])[0])
    except (TypeError, ValueError):
        return default


def _query_float(query: dict[str, list[str]], key: str, default: float) -> float:
    try:
        return float((query.get(key) or [default])[0])
    except (TypeError, ValueError):
        return default


def _header_int(value: str | None, default: int) -> int:
    try:
        return int(value or default)
    except (TypeError, ValueError):
        return default


def _required_string(payload: dict[str, Any], key: str) -> str:
    value = payload.get(key)
    if not isinstance(value, str) or not value:
        raise ValueError(f"{key} is required")
    return value


def _optional_string(payload: dict[str, Any], key: str) -> str | None:
    value = payload.get(key)
    if value is None:
        return None
    if not isinstance(value, str):
        raise ValueError(f"{key} must be a string")
    value = value.strip()
    return value or None


def _start_runtime_turn(
    runtime: Any,
    *,
    session_id: str,
    user_text: str,
    model_id: str | None,
    provider_id: str | None,
    agent_name: str | None,
    variant: str | None,
) -> Any:
    try:
        return runtime.start_turn(
            session_id=session_id,
            user_text=user_text,
            model_id=model_id,
            provider_id=provider_id,
            agent_name=agent_name,
            variant=variant,
        )
    except TypeError as error:
        if "unexpected keyword" not in str(error):
            raise
        return runtime.start_turn(session_id=session_id, user_text=user_text)


def _publish_to_control(payload: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    topic = payload.get("type") or payload.get("topic") or payload.get("event") or payload.get("method")
    if not isinstance(topic, str) or not topic:
        raise ValueError("publish type is required")
    raw_payload = payload.get("properties")
    if raw_payload is None:
        raw_payload = payload.get("payload")
    params = (
        dict(raw_payload)
        if isinstance(raw_payload, dict)
        else {key: value for key, value in payload.items() if key not in {"type", "topic", "event", "method", "properties", "payload"}}
    )
    if topic == "tui.prompt.append":
        return "prompt.append", {"text": _required_string(params, "text")}
    if topic == "tui.command.execute":
        return "command.execute", {"command": _required_string(params, "command")}
    if topic == "tui.toast.show":
        message = _required_string(params, "message")
        result: dict[str, Any] = {"message": message}
        for key in ("title", "variant", "duration"):
            if key in params and params[key] is not None:
                result[key] = params[key]
        return "toast.show", result
    if topic == "tui.session.select":
        return "session.select", {"sessionID": _required_string(params, "sessionID")}
    if topic == "tui.model.select":
        result = {"modelID": _required_string(params, "modelID")}
        if "providerID" in params and params["providerID"] is not None:
            result["providerID"] = _required_string(params, "providerID")
        return "model.select", result
    if topic == "tui.agent.select":
        return "agent.select", {"agent": _required_string(params, "agent")}
    if topic == "tui.agent.cycle":
        return "agent.cycle", {}
    if topic == "tui.variant.select":
        return "variant.select", {"variant": _required_string(params, "variant")}
    raise ValueError(f"unsupported publish type: {topic}")


if __name__ == "__main__":
    main()
