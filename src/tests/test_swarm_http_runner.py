from __future__ import annotations

import json
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from swarm import SwarmRuntime, build_http_registry, load_swarm_config


class SwarmHttpRunnerTests(unittest.IsolatedAsyncioTestCase):
    async def test_http_runner_executes_json_protocol(self) -> None:
        server = _TestHttpServer(
            {
                "/json": _json_response,
            }
        )
        self.addCleanup(server.close)
        config = _config(server.url("/json"), headers={"X-Swarm-Test": "yes"}, metadata={"team": "remote"})

        result = await SwarmRuntime(registry=build_http_registry(config), fanout_budget=config.fanout_budget).run_task(
            config.task("task"),
            run_id="http-run",
        )

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["remote"].summary, "http:Run remote HTTP.")
        self.assertEqual(result.results["remote"].evidence, ["http.py:1"])
        self.assertEqual(result.results["remote"].metadata["seen_run_id"], "http-run")
        self.assertEqual(result.results["remote"].metadata["http_status"], 200)
        self.assertEqual(result.usage.input_tokens, 4)
        self.assertEqual(result.usage.output_tokens, 5)
        self.assertEqual(server.records[0]["headers"]["X-Swarm-Test"], "yes")
        self.assertEqual(server.records[0]["payload"]["spec"]["objective"], "Run remote HTTP.")
        self.assertEqual(server.records[0]["payload"]["runner"]["metadata"]["team"], "remote")
        self.assertNotIn("headers", server.records[0]["payload"]["runner"]["metadata"])
        self.assertNotIn("url", server.records[0]["payload"]["runner"]["metadata"])
        self.assertIn("runner.started", {event.name for event in result.trace_events})
        self.assertIn("runner.finished", {event.name for event in result.trace_events})

    async def test_http_runner_accepts_plain_body_as_summary(self) -> None:
        server = _TestHttpServer({"/text": lambda _payload: (200, {}, "plain http summary")})
        self.addCleanup(server.close)
        config = _config(server.url("/text"))

        result = await SwarmRuntime(registry=build_http_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["remote"].summary, "plain http summary")
        self.assertEqual(result.results["remote"].metadata["response_format"], "text")

    async def test_http_runner_captures_http_error(self) -> None:
        server = _TestHttpServer({"/error": lambda _payload: (500, {}, "server failed")})
        self.addCleanup(server.close)
        config = _config(server.url("/error"))

        result = await SwarmRuntime(registry=build_http_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertEqual(result.results["remote"].metadata["error_kind"], "http_status_error")
        self.assertEqual(result.results["remote"].metadata["http_status"], 500)
        self.assertIn("server failed", result.results["remote"].summary)

    async def test_http_runner_times_out(self) -> None:
        server = _TestHttpServer({"/slow": _slow_response})
        self.addCleanup(server.close)
        config = _config(server.url("/slow"), timeout_seconds=0.05)

        result = await SwarmRuntime(registry=build_http_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertEqual(result.results["remote"].metadata["error_kind"], "http_timeout")


def _json_response(payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    return (
        200,
        {"Content-Type": "application/json"},
        json.dumps(
            {
                "status": "completed",
                "summary": "http:" + payload["spec"]["objective"],
                "evidence": ["http.py:1"],
                "confidence": 0.91,
                "usage": {"input_tokens": 4, "output_tokens": 5, "cost": 0.02},
                "metadata": {"seen_run_id": payload["context"]["run_id"]},
            }
        ),
    )


def _slow_response(_payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    time.sleep(0.3)
    return (200, {}, "late")


def _config(
    url: str,
    *,
    headers: dict[str, str] | None = None,
    metadata: dict[str, Any] | None = None,
    timeout_seconds: float | None = None,
):
    runner_metadata: dict[str, Any] = {
        "url": url,
        "headers": headers or {},
    }
    if metadata:
        runner_metadata.update(metadata)
    if timeout_seconds is not None:
        runner_metadata["timeout_seconds"] = timeout_seconds
    return load_swarm_config(
        {
            "runners": {
                "remote": {
                    "kind": "http",
                    "roles": ["research"],
                    "metadata": runner_metadata,
                }
            },
            "tasks": {
                "task": {
                    "role": "research",
                    "objective": "Run remote HTTP.",
                    "context": "HTTP test context.",
                    "boundaries": "Read-only.",
                    "output_schema": {"type": "object"},
                    "runner_ids": ["remote"],
                }
            },
        }
    )


class _TestHttpServer:
    def __init__(self, routes: dict[str, Any]) -> None:
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        self.server.routes = routes  # type: ignore[attr-defined]
        self.server.records = []  # type: ignore[attr-defined]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    @property
    def records(self) -> list[dict[str, Any]]:
        return self.server.records  # type: ignore[attr-defined]

    def url(self, path: str) -> str:
        host, port = self.server.server_address
        return f"http://{host}:{port}{path}"

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=1)


class _Handler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length") or 0)
        raw_body = self.rfile.read(length)
        payload = json.loads(raw_body.decode("utf-8")) if raw_body else {}
        self.server.records.append(  # type: ignore[attr-defined]
            {
                "path": self.path,
                "headers": dict(self.headers.items()),
                "payload": payload,
            }
        )
        route = self.server.routes.get(self.path)  # type: ignore[attr-defined]
        if route is None:
            status, headers, body = (404, {}, "not found")
        else:
            status, headers, body = route(payload)
        self.send_response(status)
        for key, value in headers.items():
            self.send_header(key, value)
        encoded = body.encode("utf-8")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, _format: str, *args: Any) -> None:
        return


if __name__ == "__main__":
    unittest.main()
