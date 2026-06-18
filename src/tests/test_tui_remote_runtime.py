from __future__ import annotations

import json
import tempfile
import threading
import time
import unittest
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

from openagent.app_server.protocol import AppEvent
from openagent.tui.remote_runtime import RemoteAppBridgeRuntime, RemoteTurnRecord
from openagent.tui.state import TuiState


class RemoteRuntimeServer:
    def __init__(
        self,
        *,
        required_token: str | None = None,
        global_events: list[dict[str, object]] | None = None,
        control_requests: list[dict[str, object]] | None = None,
    ) -> None:
        self.records: list[dict[str, object]] = []
        self.sessions: dict[str, dict[str, object]] = {
            "session_existing": {
                "id": "session_existing",
                "status": "ready",
                "message_count": 2,
                "messages": [
                    {"role": "user", "content": "old question"},
                    {"role": "assistant", "content": "old answer"},
                ],
            }
        }
        self.required_token = required_token
        self.global_events = global_events
        self.control_requests = list(control_requests or [])
        self.control_responses: list[dict[str, object]] = []
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self) -> None:  # noqa: N802
                if not self._authorized():
                    return
                owner.records.append({"method": "GET", "path": self.path, "authorization": self.headers.get("Authorization") or ""})
                parsed = urllib.parse.urlparse(self.path)
                if parsed.path == "/tui/control/next" and control_requests is not None:
                    request = owner.control_requests.pop(0) if owner.control_requests else None
                    self._send_json(request if isinstance(request, dict) else {"path": "", "body": None})
                    return
                if parsed.path == "/api/events" and owner.global_events is not None:
                    query = urllib.parse.parse_qs(parsed.query)
                    try:
                        last_sequence = int((query.get("last_sequence") or [self.headers.get("Last-Event-ID") or "0"])[0])
                    except ValueError:
                        last_sequence = 0
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.end_headers()
                    for event in owner.global_events:
                        global_sequence = int(event.get("global_sequence") or event.get("sequence") or 0)
                        if global_sequence <= last_sequence:
                            continue
                        self.wfile.write(
                            (
                                f"id: {global_sequence}\n"
                                f"event: {event.get('method') or 'message'}\n"
                                f"data: {json.dumps(event, ensure_ascii=False)}\n\n"
                            ).encode("utf-8")
                        )
                    return
                if self.path == "/api/sessions":
                    self._send_json({"sessions": list(owner.sessions.values())})
                    return
                if self.path == "/api/sessions/session_existing":
                    self._send_json({"session": owner.sessions["session_existing"]})
                    return
                if self.path == "/api/turns/turn_remote/events":
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.end_headers()
                    self.wfile.write(
                        (
                            'data: {"sequence": 1, "method": "turn/started", "params": {"status": "running"}}\n\n'
                            'data: {"sequence": 2, "method": "item/agentMessage/delta", "params": {"event": {"text": "hello remote"}}}\n\n'
                            'data: {"sequence": 3, "method": "turn/approval_requested", "params": {"status": "waiting_approval", "approval": {"turn_id": "turn_remote", "request_id": "approval_1", "tool_name": "write", "tool_input": {"path": "a.txt"}}}}\n\n'
                            'data: {"sequence": 4, "method": "turn/completed", "params": {"status": "completed", "final_answer": "hello remote", "trace": {"id": "trace_1"}}}\n\n'
                        ).encode("utf-8")
                    )
                    return
                self._send_json({"error": "not found"}, status=404)

            def do_POST(self) -> None:  # noqa: N802
                if not self._authorized():
                    return
                payload = self._read_json()
                owner.records.append({"method": "POST", "path": self.path, "payload": payload, "authorization": self.headers.get("Authorization") or ""})
                if self.path == "/tui/control/response":
                    owner.control_responses.append(payload)
                    self._send_json({"ok": True, "response": payload})
                    return
                if self.path == "/api/sessions":
                    session = {"id": "session_new", "status": "ready", "message_count": 0, "messages": []}
                    owner.sessions["session_new"] = session
                    self._send_json({"session": session}, status=201)
                    return
                if self.path == "/api/sessions/session_existing/rename":
                    session = dict(owner.sessions["session_existing"])
                    session["title"] = payload.get("title")
                    owner.sessions["session_existing"] = session
                    self._send_json({"session": session})
                    return
                if self.path == "/api/sessions/session_existing/archive":
                    session = dict(owner.sessions["session_existing"])
                    session["archived"] = True
                    owner.sessions["session_existing"] = session
                    self._send_json({"session": session})
                    return
                if self.path == "/api/sessions/session_existing/fork":
                    session = {
                        "id": "session_fork",
                        "status": "ready",
                        "message_count": 2,
                        "title": payload.get("title") or "Fork",
                        "forked_from": "session_existing",
                    }
                    owner.sessions["session_fork"] = session
                    self._send_json({"session": session}, status=201)
                    return
                if self.path == "/api/sessions/session_existing/turns":
                    self._send_json({"turn": {"id": "turn_remote", "session_id": "session_existing", "status": "queued"}}, status=201)
                    return
                if self.path == "/api/turns/turn_remote/interrupt":
                    self._send_json({"turn": {"id": "turn_remote", "status": "interrupting"}})
                    return
                if self.path == "/api/turns/turn_remote/patches/last/revert":
                    self._send_json(
                        {
                            "event": {
                                "sequence": 6,
                                "method": "item/patch/reverted",
                                "params": {
                                    "thread_id": "session_existing",
                                    "turn_id": "turn_remote",
                                    "patch_hash": "hash_123",
                                    "target": payload.get("target"),
                                    "reverted": ["a.txt: restored"],
                                    "skipped": [],
                                },
                            }
                        }
                    )
                    return
                if self.path == "/api/turns/turn_remote/approvals/approval_1":
                    self._send_json(
                        {
                            "event": {
                                "sequence": 5,
                                "method": "turn/approval_resolved",
                                "params": {
                                    "approval": {
                                        "request_id": "approval_1",
                                        "action": payload.get("action"),
                                        "scope": payload.get("scope"),
                                        "note": payload.get("note"),
                                    }
                                },
                            }
                        }
                    )
                    return
                self._send_json({"error": "not found"}, status=404)

            def log_message(self, format: str, *args: object) -> None:  # noqa: A002
                return

            def _authorized(self) -> bool:
                if owner.required_token is None:
                    return True
                if self.headers.get("Authorization") == f"Bearer {owner.required_token}":
                    return True
                self._send_json({"error": "unauthorized"}, status=401)
                return False

            def _read_json(self) -> dict[str, object]:
                raw_len = int(self.headers.get("Content-Length") or "0")
                if raw_len <= 0:
                    return {}
                value = json.loads(self.rfile.read(raw_len).decode("utf-8"))
                return value if isinstance(value, dict) else {}

            def _send_json(self, payload: dict[str, object], *, status: int = 200) -> None:
                data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        host, port = self.server.server_address
        self.url = f"http://{host}:{port}"

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


class RemoteAppBridgeRuntimeTests(unittest.TestCase):
    def test_remote_runtime_reports_auth_failure_without_token(self) -> None:
        server = RemoteRuntimeServer(required_token="secret")
        self.addCleanup(server.close)
        runtime = RemoteAppBridgeRuntime(server_url=server.url)

        with self.assertRaisesRegex(Exception, "401|unauthorized"):
            runtime.list_sessions()

    def test_remote_runtime_sessions_turn_sse_interrupt_approval_and_token_headers(self) -> None:
        server = RemoteRuntimeServer(required_token="secret")
        self.addCleanup(server.close)
        with tempfile.TemporaryDirectory() as raw_tmp:
            workspace = Path(raw_tmp).resolve()
            runtime = RemoteAppBridgeRuntime(server_url=server.url + "/", workspace=workspace, auth_token="secret")

            sessions = runtime.list_sessions()
            resumed = runtime.resume_session("session_existing")
            fetched = runtime.get_session("session_existing")
            created = runtime.start_session()
            turn = runtime.start_turn(session_id="session_existing", user_text="hello")
            approval_response = runtime.respond_approval("turn_remote", "approval_1", "deny")
            interrupt_response = runtime.interrupt_turn("turn_remote")

            self.assertTrue(turn.wait_until_terminal(timeout_s=5.0))

        self.assertEqual(sessions[0]["id"], "session_existing")
        self.assertEqual(resumed["messages"][0]["content"], "old question")
        self.assertEqual(fetched["id"], "session_existing")
        self.assertEqual(created["id"], "session_new")
        self.assertEqual(turn.status, "completed")
        self.assertEqual(turn.final_answer, "hello remote")
        self.assertEqual(turn.trace, {"id": "trace_1"})
        self.assertEqual([event.method for event in turn.events], ["turn/started", "item/agentMessage/delta", "turn/approval_requested", "turn/completed"])
        self.assertEqual(approval_response["method"], "turn/approval_resolved")
        self.assertEqual(interrupt_response["status"], "interrupting")
        self.assertTrue(server.records)
        self.assertTrue(all(record["authorization"] == "Bearer secret" for record in server.records))
        create_record = next(record for record in server.records if record["path"] == "/api/sessions" and record["method"] == "POST")
        self.assertEqual(create_record["payload"]["cwd"], str(workspace))

    def test_remote_runtime_posts_patch_revert_and_routes_event(self) -> None:
        server = RemoteRuntimeServer()
        self.addCleanup(server.close)
        runtime = RemoteAppBridgeRuntime(server_url=server.url, use_global_events=False)
        turn = RemoteTurnRecord(id="turn_remote", session_id="session_existing", status="completed")
        runtime._turns["turn_remote"] = turn  # noqa: SLF001 - regression covers local routing of returned server event.

        response = runtime.revert_patch("turn_remote", "last", target="a.txt")

        revert_record = next(record for record in server.records if record["path"] == "/api/turns/turn_remote/patches/last/revert")
        self.assertEqual(revert_record["payload"], {"target": "a.txt"})
        self.assertEqual(response["method"], "item/patch/reverted")
        self.assertEqual(turn.events[-1].method, "item/patch/reverted")
        self.assertEqual(turn.events[-1].params["reverted"], ["a.txt: restored"])

    def test_remote_runtime_posts_approval_scope_and_note(self) -> None:
        server = RemoteRuntimeServer()
        self.addCleanup(server.close)
        runtime = RemoteAppBridgeRuntime(server_url=server.url)

        response = runtime.respond_approval("turn_remote", "approval_1", "allow", scope="always", note="trusted")

        approval_record = next(record for record in server.records if record["path"] == "/api/turns/turn_remote/approvals/approval_1")
        self.assertEqual(approval_record["payload"], {"action": "allow", "scope": "always", "note": "trusted"})
        self.assertEqual(response["params"]["approval"]["scope"], "always")
        self.assertEqual(response["params"]["approval"]["note"], "trusted")

    def test_remote_runtime_session_manager_posts_endpoints(self) -> None:
        server = RemoteRuntimeServer()
        self.addCleanup(server.close)
        runtime = RemoteAppBridgeRuntime(server_url=server.url)

        renamed = runtime.rename_session("session_existing", "Main")
        forked = runtime.fork_session("session_existing", title="Branch")
        archived = runtime.archive_session("session_existing")

        paths = [record["path"] for record in server.records if record["method"] == "POST"]
        self.assertIn("/api/sessions/session_existing/rename", paths)
        self.assertIn("/api/sessions/session_existing/fork", paths)
        self.assertIn("/api/sessions/session_existing/archive", paths)
        self.assertEqual(renamed["title"], "Main")
        self.assertEqual(forked["forked_from"], "session_existing")
        self.assertEqual(archived["archived"], True)

    def test_remote_runtime_consumes_global_events_for_remote_turn(self) -> None:
        server = RemoteRuntimeServer(
            global_events=[
                {
                    "sequence": 1,
                    "global_sequence": 1,
                    "method": "turn/started",
                    "params": {"thread_id": "session_existing", "turn_id": "turn_global", "status": "running"},
                },
                {
                    "sequence": 2,
                    "global_sequence": 2,
                    "method": "item/agentMessage/delta",
                    "params": {"thread_id": "session_existing", "turn_id": "turn_global", "event": {"text": "from global"}},
                },
                {
                    "sequence": 3,
                    "global_sequence": 3,
                    "method": "turn/completed",
                    "params": {"thread_id": "session_existing", "turn_id": "turn_global", "status": "completed", "final_answer": "from global"},
                },
            ]
        )
        self.addCleanup(server.close)
        runtime = RemoteAppBridgeRuntime(server_url=server.url)

        turn = _wait_for_remote_turn(runtime, "turn_global")
        self.assertTrue(turn.wait_until_terminal(timeout_s=5.0))

        self.assertEqual(turn.session_id, "session_existing")
        self.assertEqual(turn.status, "completed")
        self.assertEqual(turn.final_answer, "from global")
        self.assertEqual([event.method for event in turn.events], ["turn/started", "item/agentMessage/delta", "turn/completed"])
        self.assertFalse(any(record["path"] == "/api/turns/turn_global/events" for record in server.records))

    def test_remote_runtime_polls_tui_control_with_auth_and_posts_response(self) -> None:
        server = RemoteRuntimeServer(
            required_token="secret",
            control_requests=[
                {"path": "/tui/append-prompt", "body": {"text": "hello"}},
            ],
        )
        self.addCleanup(server.close)
        runtime = RemoteAppBridgeRuntime(server_url=server.url, auth_token="secret")

        requests = _wait_for_control_requests(runtime)
        response = runtime.post_control_response({"path": "/tui/append-prompt"}, ok=True, result={"applied": True})

        self.assertEqual(requests, [{"path": "/tui/append-prompt", "body": {"text": "hello"}}])
        self.assertEqual(response["ok"], True)
        control_get = next(record for record in server.records if str(record["path"]).startswith("/tui/control/next"))
        control_post = next(record for record in server.records if record["path"] == "/tui/control/response")
        self.assertEqual(control_get["authorization"], "Bearer secret")
        self.assertEqual(control_post["authorization"], "Bearer secret")
        self.assertEqual(server.control_responses[0]["path"], "/tui/append-prompt")
        self.assertEqual(server.control_responses[0]["result"], {"applied": True})

    def test_start_turn_reuses_existing_global_stream_record(self) -> None:
        runtime = RemoteAppBridgeRuntime(server_url="http://127.0.0.1:9", use_global_events=False)
        runtime._route_global_event(  # noqa: SLF001 - regression covers internal global-stream routing race.
            AppEvent(
                sequence=1,
                global_sequence=1,
                method="turn/started",
                params={"thread_id": "session_existing", "turn_id": "turn_race", "status": "running"},
            )
        )
        existing = runtime.get_turn("turn_race")

        with (
            patch(
                "openagent.tui.remote_runtime.app_bridge_post_json",
                return_value={"turn": {"id": "turn_race", "session_id": "session_existing", "status": "queued"}},
            ),
            patch.object(runtime, "_should_start_turn_stream", return_value=False),
        ):
            turn = runtime.start_turn(session_id="session_existing", user_text="hello")

        self.assertIs(turn, existing)
        self.assertEqual(turn.status, "running")
        self.assertEqual([event.method for event in turn.events], ["turn/started"])

    def test_remote_turn_record_deduplicates_replayed_global_events(self) -> None:
        turn = RemoteTurnRecord(id="turn_remote", session_id="session_existing")
        event = AppEvent(
            sequence=1,
            global_sequence=11,
            method="turn/started",
            params={"thread_id": "session_existing", "turn_id": "turn_remote", "status": "running"},
        )
        duplicate = AppEvent(
            sequence=1,
            global_sequence=11,
            method="turn/started",
            params={"thread_id": "session_existing", "turn_id": "turn_remote", "status": "running"},
        )

        self.assertTrue(turn.append_event(event))
        self.assertFalse(turn.append_event(duplicate))

        self.assertEqual(len(turn.events), 1)

    def test_tui_state_sends_remote_approval_and_interrupt(self) -> None:
        server = RemoteRuntimeServer()
        self.addCleanup(server.close)
        runtime = RemoteAppBridgeRuntime(server_url=server.url)
        state = TuiState(runtime=runtime)
        state.active_turn = RemoteTurnRecord(id="turn_remote", session_id="session_existing", status="waiting_approval")
        state.active_approval = {
            "turn_id": "turn_remote",
            "request_id": "approval_1",
            "tool_name": "write",
            "tool_input": {"path": "a.txt"},
        }

        self.assertTrue(state.respond_approval("allow"))
        state.request_interrupt()

        approval_record = next(record for record in server.records if record["path"] == "/api/turns/turn_remote/approvals/approval_1")
        interrupt_record = next(record for record in server.records if record["path"] == "/api/turns/turn_remote/interrupt")
        self.assertEqual(approval_record["payload"], {"action": "allow"})
        self.assertEqual(interrupt_record["payload"], {})
        self.assertEqual(state.status, "interrupting")

    def test_tui_state_polls_remote_turn_events(self) -> None:
        server = RemoteRuntimeServer()
        self.addCleanup(server.close)
        runtime = RemoteAppBridgeRuntime(server_url=server.url)
        state = TuiState(runtime=runtime)
        state.session_id = "session_existing"
        state.input_buffer = "hello"

        self.assertTrue(state.submit())
        assert state.active_turn is not None
        self.assertTrue(state.active_turn.wait_until_terminal(timeout_s=5.0))
        state.poll_events()

        timeline_text = "\n".join(line.text for line in state.timeline)
        self.assertEqual(state.status, "completed")
        self.assertIn("> hello", timeline_text)
        self.assertIn("hello remote", timeline_text)
        self.assertIn("approval required: write", timeline_text)


def _wait_for_remote_turn(runtime: RemoteAppBridgeRuntime, turn_id: str, *, timeout_s: float = 5.0) -> RemoteTurnRecord:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            return runtime.get_turn(turn_id)
        except KeyError:
            time.sleep(0.02)
    raise AssertionError(f"remote turn was not routed from global stream: {turn_id}")


def _wait_for_control_requests(runtime: RemoteAppBridgeRuntime, *, timeout_s: float = 5.0) -> list[dict[str, object]]:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        requests = runtime.drain_control_requests()
        if requests:
            return requests
        time.sleep(0.02)
    raise AssertionError("remote TUI control request was not polled")


if __name__ == "__main__":
    unittest.main()
