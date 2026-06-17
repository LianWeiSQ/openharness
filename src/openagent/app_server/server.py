from __future__ import annotations

import argparse
import json
import mimetypes
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

from .runtime import OpenAgentAppRuntime

STATIC_DIR = Path(__file__).resolve().parent / "static"


class OpenAgentAppRequestHandler(BaseHTTPRequestHandler):
    runtime: OpenAgentAppRuntime
    serve_static: bool = True

    server_version = "OpenAgentAppServer/0.1"

    def do_OPTIONS(self) -> None:  # noqa: N802
        self.send_response(HTTPStatus.NO_CONTENT)
        self._common_headers()
        self.end_headers()

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        path = parsed.path
        try:
            if path == "/api/health":
                self._send_json({"ok": True, "service": "openagent-app-server", "ui_enabled": self.serve_static})
            elif path == "/api/models":
                self._send_json({"models": self.runtime.list_models()})
            elif path == "/api/sessions":
                self._send_json({"sessions": self.runtime.list_sessions()})
            elif path.startswith("/api/sessions/"):
                session_id = path.removeprefix("/api/sessions/").strip("/")
                if not session_id:
                    self._send_error(HTTPStatus.NOT_FOUND, "session id is required")
                else:
                    self._send_json({"session": self.runtime.get_session(session_id)})
            elif path.startswith("/api/turns/") and path.endswith("/events"):
                turn_id = path.removeprefix("/api/turns/").removesuffix("/events").strip("/")
                query = parse_qs(parsed.query)
                last_sequence = _query_int(query, "last_sequence", 0)
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

    def _stream_turn_events(self, turn_id: str, *, last_sequence: int) -> None:
        turn = self.runtime.get_turn(turn_id)
        self.send_response(HTTPStatus.OK)
        self._common_headers(content_type="text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.end_headers()

        sequence = max(1, last_sequence + 1)
        while True:
            event = turn.wait_for_sequence(sequence, timeout_s=10.0)
            if event is None:
                self._write_sse_comment("ping")
                if turn.status in {"completed", "failed", "interrupted"} and sequence > len(turn.events):
                    break
                continue
            self._write_sse(event.method, event.to_dict(), event_id=str(event.sequence))
            sequence = event.sequence + 1
            if event.method in {"turn/completed", "turn/failed"}:
                break

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
        self.send_header("Access-Control-Allow-Headers", "Content-Type, Last-Event-ID")

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
) -> ThreadingHTTPServer:
    runtime = OpenAgentAppRuntime(workspace=workspace, session_store_root=session_store_root)

    class Handler(OpenAgentAppRequestHandler):
        pass

    Handler.runtime = runtime
    Handler.serve_static = serve_static
    return ThreadingHTTPServer((host, port), Handler)


def serve(
    *,
    host: str,
    port: int,
    workspace: str | Path | None = None,
    session_store_root: str | Path | None = None,
    serve_static: bool = True,
) -> ThreadingHTTPServer:
    httpd = create_server(host=host, port=port, workspace=workspace, session_store_root=session_store_root, serve_static=serve_static)
    mode = "console" if serve_static else "headless"
    print(f"OpenAgent app bridge ({mode}) listening on http://{host}:{port}")  # noqa: T201
    httpd.serve_forever()
    return httpd


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description="Run the local OpenAgent App Bridge UI.")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8787)
    parser.add_argument("--workspace", default=None)
    parser.add_argument("--session-root", default=None)
    parser.add_argument("--headless", action="store_true", help="serve API/SSE endpoints without the static console")
    args = parser.parse_args(argv)
    serve(host=args.host, port=args.port, workspace=args.workspace, session_store_root=args.session_root, serve_static=not args.headless)


def _query_int(query: dict[str, list[str]], key: str, default: int) -> int:
    try:
        return int((query.get(key) or [default])[0])
    except (TypeError, ValueError):
        return default


if __name__ == "__main__":
    main()
