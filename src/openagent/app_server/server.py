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
        if _is_api_path(path) and not self._authorize_api_request():
            return
        try:
            if path == "/api/health":
                self._send_json({"ok": True, "service": "openagent-app-server", "ui_enabled": self.serve_static, "auth_required": bool(self.auth_token)})
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
        except KeyError as error:
            self._send_error(HTTPStatus.NOT_FOUND, str(error))
        except Exception as error:  # noqa: BLE001
            self._send_error(HTTPStatus.INTERNAL_SERVER_ERROR, str(error))

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        path = parsed.path
        if _is_api_path(path) and not self._authorize_api_request():
            return
        try:
            payload = self._read_json()
            if path == "/api/sessions":
                session = self.runtime.start_session(cwd=payload.get("cwd"))
                self._send_json({"session": session}, status=HTTPStatus.CREATED)
            elif path.startswith("/api/sessions/") and path.endswith("/turns"):
                session_id = path.removeprefix("/api/sessions/").removesuffix("/turns").strip("/")
                user_text = str(payload.get("input") or payload.get("user_text") or "")
                turn = self.runtime.start_turn(session_id=session_id, user_text=user_text)
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
        except KeyError as error:
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
        raw_len = int(self.headers.get("Content-Length") or "0")
        if raw_len <= 0:
            return {}
        raw = self.rfile.read(raw_len).decode("utf-8")
        value = json.loads(raw)
        if not isinstance(value, dict):
            raise ValueError("JSON object body is required")
        return value

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


def _is_api_path(path: str) -> bool:
    return path.startswith("/api/")


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


def _header_int(value: str | None, default: int) -> int:
    try:
        return int(value or default)
    except (TypeError, ValueError):
        return default


if __name__ == "__main__":
    main()
