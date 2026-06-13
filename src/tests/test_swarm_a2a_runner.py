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

    async def test_a2a_runner_streams_task_artifact_and_status_updates(self) -> None:
        server = _TestA2AServer({"/message:stream": _completed_stream})
        self.addCleanup(server.close)
        config = _config(server.url("/"), streaming=True)

        result = await SwarmRuntime(registry=build_a2a_registry(config)).run_task(config.task("task"), run_id="stream-run")

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["remote"].summary, "# Report\n\nDone")
        self.assertEqual(result.results["remote"].evidence, ["final"])
        self.assertEqual(result.results["remote"].metadata["response_format"], "a2a-sse")
        self.assertEqual(result.results["remote"].metadata["a2a_stream_events"], 3)
        self.assertEqual(result.results["remote"].metadata["a2a_task_id"], "task-stream-1")
        self.assertEqual(result.results["remote"].metadata["a2a_task_state"], "TASK_STATE_COMPLETED")
        self.assertEqual(server.records[0]["path"], "/message:stream")
        self.assertEqual(server.records[0]["headers"]["Accept"], "text/event-stream")
        event_names = [event.name for event in result.trace_events]
        self.assertIn("a2a.stream.task", event_names)
        self.assertIn("a2a.stream.artifactUpdate", event_names)
        self.assertIn("a2a.stream.statusUpdate", event_names)

    async def test_a2a_runner_streams_direct_message_response(self) -> None:
        server = _TestA2AServer({"/message:stream": _message_stream})
        self.addCleanup(server.close)
        config = _config(server.url("/message:send"), streaming=True)

        result = await SwarmRuntime(registry=build_a2a_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["remote"].summary, "Direct stream answer")
        self.assertEqual(server.records[0]["path"], "/message:stream")

    async def test_a2a_runner_streams_failed_status(self) -> None:
        server = _TestA2AServer({"/message:stream": _failed_stream})
        self.addCleanup(server.close)
        config = _config(server.url("/"), streaming=True)

        result = await SwarmRuntime(registry=build_a2a_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertEqual(result.results["remote"].status, "failed")
        self.assertEqual(result.results["remote"].summary, "Stream failed")
        self.assertEqual(result.results["remote"].metadata["a2a_task_state"], "TASK_STATE_FAILED")

    async def test_a2a_runner_subscribes_to_task_from_metadata(self) -> None:
        server = _TestA2AServer({"/tasks/task-subscribe-1:subscribe": _subscribed_stream})
        self.addCleanup(server.close)
        config = _config(server.url("/"), subscribe_task_id="task-subscribe-1")

        result = await SwarmRuntime(registry=build_a2a_registry(config)).run_task(config.task("task"), run_id="subscribe-run")

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["remote"].summary, "Subscribed result")
        self.assertEqual(result.results["remote"].evidence, ["final"])
        self.assertEqual(result.results["remote"].metadata["response_format"], "a2a-sse")
        self.assertEqual(result.results["remote"].metadata["a2a_task_id"], "task-subscribe-1")
        self.assertEqual(result.results["remote"].metadata["a2a_subscribed_task_id"], "task-subscribe-1")
        self.assertEqual(result.results["remote"].metadata["a2a_task_state"], "TASK_STATE_COMPLETED")
        self.assertEqual(server.records[0]["path"], "/tasks/task-subscribe-1:subscribe")
        self.assertEqual(server.records[0]["headers"]["Accept"], "text/event-stream")
        self.assertEqual(server.records[0]["headers"]["A2A-Version"], "1.0")
        self.assertEqual(server.records[0]["payload"]["id"], "task-subscribe-1")
        event_names = [event.name for event in result.trace_events]
        self.assertIn("a2a.stream.task", event_names)
        self.assertIn("a2a.stream.artifactUpdate", event_names)
        self.assertIn("a2a.stream.statusUpdate", event_names)

    async def test_a2a_runner_subscribe_task_id_can_come_from_task_inputs(self) -> None:
        server = _TestA2AServer({"/api/tasks/task-input-1:subscribe": _subscribed_input_stream})
        self.addCleanup(server.close)
        config = _config(
            server.url("/api/message:send"),
            subscribe_task_id="runner-default",
            task_inputs={"topic": "interoperability", "a2a_task_id": "task-input-1"},
        )

        result = await SwarmRuntime(registry=build_a2a_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.results["remote"].summary, "Task input subscription result")
        self.assertEqual(result.results["remote"].metadata["a2a_subscribed_task_id"], "task-input-1")
        self.assertEqual(server.records[0]["path"], "/api/tasks/task-input-1:subscribe")

    async def test_a2a_runner_subscribes_failed_task_to_failed_result(self) -> None:
        server = _TestA2AServer({"/tasks/task-subscribe-failed:subscribe": _subscribed_failed_stream})
        self.addCleanup(server.close)
        config = _config(server.url("/"), subscribe_task_id="task-subscribe-failed")

        result = await SwarmRuntime(registry=build_a2a_registry(config)).run_task(config.task("task"))

        self.assertEqual(result.status, "failed")
        self.assertEqual(result.results["remote"].status, "failed")
        self.assertEqual(result.results["remote"].summary, "Subscribed task failed")
        self.assertEqual(result.results["remote"].metadata["a2a_task_state"], "TASK_STATE_FAILED")


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


def _completed_stream(_payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    return (
        200,
        {"Content-Type": "text/event-stream"},
        _sse(
            {"task": {"id": "task-stream-1", "status": {"state": "TASK_STATE_WORKING"}}},
            {"artifactUpdate": {"taskId": "task-stream-1", "artifact": {"name": "final", "parts": [{"text": "# Report\n\nDone"}]}}},
            {"statusUpdate": {"taskId": "task-stream-1", "status": {"state": "TASK_STATE_COMPLETED"}}},
        ),
    )


def _message_stream(_payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    return (
        200,
        {"Content-Type": "text/event-stream"},
        _sse({"message": {"role": "ROLE_AGENT", "parts": [{"text": "Direct stream answer"}]}}),
    )


def _failed_stream(_payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    return (
        200,
        {"Content-Type": "text/event-stream"},
        _sse(
            {"task": {"id": "task-stream-2", "status": {"state": "TASK_STATE_WORKING"}}},
            {
                "statusUpdate": {
                    "taskId": "task-stream-2",
                    "status": {
                        "state": "TASK_STATE_FAILED",
                        "message": {"role": "ROLE_AGENT", "parts": [{"text": "Stream failed"}]},
                    },
                }
            },
        ),
    )


def _subscribed_stream(_payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    return (
        200,
        {"Content-Type": "text/event-stream"},
        _sse(
            {"task": {"id": "task-subscribe-1", "status": {"state": "TASK_STATE_WORKING"}}},
            {
                "artifactUpdate": {
                    "taskId": "task-subscribe-1",
                    "artifact": {"name": "final", "parts": [{"text": "Subscribed result"}]},
                }
            },
            {"statusUpdate": {"taskId": "task-subscribe-1", "status": {"state": "TASK_STATE_COMPLETED"}}},
        ),
    )


def _subscribed_input_stream(_payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    return (
        200,
        {"Content-Type": "text/event-stream"},
        _sse(
            {"task": {"id": "task-input-1", "status": {"state": "TASK_STATE_WORKING"}}},
            {
                "artifactUpdate": {
                    "taskId": "task-input-1",
                    "artifact": {"name": "final", "parts": [{"text": "Task input subscription result"}]},
                }
            },
            {"statusUpdate": {"taskId": "task-input-1", "status": {"state": "TASK_STATE_COMPLETED"}}},
        ),
    )


def _subscribed_failed_stream(_payload: dict[str, Any]) -> tuple[int, dict[str, str], str]:
    return (
        200,
        {"Content-Type": "text/event-stream"},
        _sse(
            {"task": {"id": "task-subscribe-failed", "status": {"state": "TASK_STATE_WORKING"}}},
            {
                "statusUpdate": {
                    "taskId": "task-subscribe-failed",
                    "status": {
                        "state": "TASK_STATE_FAILED",
                        "message": {"role": "ROLE_AGENT", "parts": [{"text": "Subscribed task failed"}]},
                    },
                }
            },
        ),
    )


def _sse(*events: dict[str, Any]) -> str:
    return "".join(f"data: {json.dumps(event)}\n\n" for event in events)


def _config(
    url: str,
    *,
    headers: dict[str, str] | None = None,
    version: str = "1.0",
    streaming: bool = False,
    subscribe_task_id: str | None = None,
    task_inputs: dict[str, Any] | None = None,
    task_metadata: dict[str, Any] | None = None,
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
                        "streaming": streaming,
                        "subscribe_task_id": subscribe_task_id,
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
                    "inputs": task_inputs or {"topic": "interoperability"},
                    "metadata": task_metadata or {},
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
