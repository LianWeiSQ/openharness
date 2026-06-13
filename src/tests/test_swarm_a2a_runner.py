from __future__ import annotations

import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from swarm import SwarmRuntime, build_a2a_registry, load_swarm_config


class SwarmA2ARunnerTests(unittest.IsolatedAsyncioTestCase):
    async def test_a2a_runner_sends_message_and_parses_completed_task(self) -> None:
        server = _TestA2AServer({"/message:send": _completed_task})
        self.addCleanup(server.close)
        config = _config(server.url("/"), headers={"Authorization": "Bearer test"}, version="1.0")

        result = await SwarmRuntime(registry=build_a2a_registry(config)).run_task(config.task("task"), run_id="a2a-run")

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["remote"].summary, "A2A completed result")
        self.assertEqual(result.results["remote"].evidence, ["final"])
        self.assertEqual(result.results["remote"].metadata["a2a_task_id"], "task-1")
        self.assertEqual(result.results["remote"].metadata["a2a_task_state"], "TASK_STATE_COMPLETED")
        self.assertEqual(server.records[0]["path"], "/message:send")
        self.assertEqual(server.records[0]["headers"]["A2A-Version"], "1.0")
        self.assertEqual(server.records[0]["headers"]["Authorization"], "Bearer test")
        self.assertTrue(server.records[0]["headers"]["Content-Type"].startswith("application/a2a+json"))
        payload = server.records[0]["payload"]
        self.assertEqual(payload["message"]["role"], "ROLE_USER")
        self.assertEqual(payload["message"]["contextId"], "a2a-run")
        self.assertIn("Objective: Run remote A2A.", payload["message"]["parts"][0]["text"])
        self.assertEqual(payload["configuration"]["acceptedOutputModes"], ["text/plain"])
        self.assertIn("runner.started", {event.name for event in result.trace_events})
        self.assertIn("runner.finished", {event.name for event in result.trace_events})

    async def test_a2a_runner_maps_input_required_to_partial(self) -> None:
        server = _TestA2AServer({"/message:send": _input_required_task})
        self.addCleanup(server.close)
        config = _config(server.url("/message:send"))

        result = await SwarmRuntime(registry=build_a2a_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "partial")
        self.assertEqual(result.results["remote"].status, "partial")
        self.assertEqual(result.results["remote"].summary, "Need more detail")
        self.assertEqual(result.results["remote"].metadata["a2a_task_state"], "TASK_STATE_INPUT_REQUIRED")

    async def test_a2a_runner_maps_failed_task_to_failed_result(self) -> None:
        server = _TestA2AServer({"/message:send": _failed_task})
        self.addCleanup(server.close)
        config = _config(server.url("/"))

        result = await SwarmRuntime(registry=build_a2a_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertEqual(result.results["remote"].status, "failed")
        self.assertEqual(result.results["remote"].summary, "Remote task failed")
        self.assertEqual(result.results["remote"].metadata["a2a_task_state"], "TASK_STATE_FAILED")

    async def test_a2a_runner_captures_http_error(self) -> None:
        server = _TestA2AServer({"/message:send": lambda _payload: (500, {}, "a2a failed")})
        self.addCleanup(server.close)
        config = _config(server.url("/"))

        result = await SwarmRuntime(registry=build_a2a_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertEqual(result.results["remote"].metadata["error_kind"], "a2a_http_status_error")
        self.assertEqual(result.results["remote"].metadata["http_status"], 500)
        self.assertIn("a2a failed", result.results["remote"].summary)


def _completed_task(_payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    return (
        200,
        {"Content-Type": "application/a2a+json"},
        json.dumps(
            {
                "task": {
                    "id": "task-1",
                    "status": {"state": "TASK_STATE_COMPLETED"},
                    "artifacts": [
                        {
                            "artifactId": "final",
                            "name": "final",
                            "parts": [{"text": "A2A completed result"}],
                        }
                    ],
                }
            }
        ),
    )


def _input_required_task(_payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    return (
        200,
        {"Content-Type": "application/a2a+json"},
        json.dumps(
            {
                "task": {
                    "id": "task-2",
                    "status": {
                        "state": "TASK_STATE_INPUT_REQUIRED",
                        "message": {"role": "ROLE_AGENT", "parts": [{"text": "Need more detail"}]},
                    },
                }
            }
        ),
    )


def _failed_task(_payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    return (
        200,
        {"Content-Type": "application/a2a+json"},
        json.dumps(
            {
                "task": {
                    "id": "task-3",
                    "status": {
                        "state": "TASK_STATE_FAILED",
                        "message": {"role": "ROLE_AGENT", "parts": [{"text": "Remote task failed"}]},
                    },
                }
            }
        ),
    )


def _config(
    url: str,
    *,
    headers: dict[str, str] | None = None,
    version: str = "1.0",
):
    return load_swarm_config(
        {
            "runners": {
                "remote": {
                    "kind": "a2a",
                    "roles": ["research"],
                    "metadata": {
                        "url": url,
                        "headers": headers or {},
                        "version": version,
                        "accepted_output_modes": ["text/plain"],
                    },
                }
            },
            "tasks": {
                "task": {
                    "role": "research",
                    "objective": "Run remote A2A.",
                    "context": "A2A test context.",
                    "boundaries": "Read-only.",
                    "output_schema": {"type": "object"},
                    "runner_ids": ["remote"],
                    "inputs": {"topic": "interoperability"},
                }
            },
        }
    )


class _TestA2AServer:
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
