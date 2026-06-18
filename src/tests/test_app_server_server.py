from __future__ import annotations

import json
import shutil
import threading
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from uuid import uuid4

from openagent.app_server.protocol import AppEvent, TuiControlRequest
from openagent.app_server.server import create_server


class FakeRuntime:
    def __init__(self) -> None:
        self.global_events = [
            AppEvent(
                sequence=1,
                method="turn/started",
                params={"thread_id": "session_1", "turn_id": "turn_1", "status": "running"},
                global_sequence=1,
            ),
            AppEvent(
                sequence=2,
                method="turn/completed",
                params={"thread_id": "session_1", "turn_id": "turn_1", "status": "completed", "final_answer": "done"},
                global_sequence=2,
            ),
            AppEvent(
                sequence=1,
                method="turn/started",
                params={"thread_id": "session_1", "turn_id": "turn_2", "status": "running"},
                global_sequence=3,
            ),
        ]
        self.control_requests: list[TuiControlRequest] = []
        self.control_responses: list[object] = []
        self.reverts: list[tuple[str, str, str]] = []
        self.sessions = [{"id": "session_existing", "status": "ready", "message_count": 0}]

    def list_models(self):
        return []

    def list_sessions(self):
        return [dict(item) for item in self.sessions if not item.get("archived")]

    def get_session(self, session_id: str):
        for session in self.sessions:
            if session["id"] == session_id:
                return dict(session)
        raise KeyError(f"Unknown session: {session_id}")

    def rename_session(self, session_id: str, title: str):
        session = self.get_session(session_id)
        session["title"] = title
        self._replace_session(session)
        return session

    def archive_session(self, session_id: str):
        session = self.get_session(session_id)
        session["archived"] = True
        self._replace_session(session)
        return session

    def fork_session(self, session_id: str, *, title: str | None = None):
        self.get_session(session_id)
        session = {"id": "session_fork", "status": "ready", "message_count": 0, "title": title or "Fork", "forked_from": session_id}
        self.sessions.append(session)
        return session

    def _replace_session(self, session):
        self.sessions = [session if item["id"] == session["id"] else item for item in self.sessions]

    def interrupt_turn(self, turn_id: str):
        return {"id": turn_id, "status": "interrupting", "interrupt_requested": True}

    def respond_approval(self, turn_id: str, request_id: str, action: str, *, scope: str | None = None, note: str | None = None):
        approval = {
            "request_id": request_id,
            "action": action,
        }
        if scope:
            approval["scope"] = scope
        if note:
            approval["note"] = note
        return {
            "method": "turn/approval_resolved",
            "params": {
                "turn_id": turn_id,
                "approval": approval,
            },
        }

    def revert_patch(self, turn_id: str, patch_ref: str = "last", *, target: str = "all"):
        self.reverts.append((turn_id, patch_ref, target))
        return {
            "method": "item/patch/reverted",
            "params": {
                "turn_id": turn_id,
                "patch_hash": "hash_123",
                "target": target,
                "reverted": ["a.txt: restored"],
                "skipped": [],
            },
        }

    def wait_for_global_sequence(self, sequence: int, *, timeout_s: float = 15.0):
        if 1 <= sequence <= len(self.global_events):
            return self.global_events[sequence - 1]
        return None

    def enqueue_tui_control(self, path: str, body: object | None = None):
        request = TuiControlRequest(path=path, body={} if body is None else body)
        self.control_requests.append(request)
        return request

    def wait_for_tui_control(self, *, timeout_s: float = 0.25):
        del timeout_s
        if not self.control_requests:
            return None
        return self.control_requests.pop(0)

    def record_tui_control_response(self, payload: object | None = None):
        self.control_responses.append(payload)
        return payload


class AppServerServerTests(unittest.TestCase):
    def _make_temp_dir(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"server_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, td, True)
        return td

    def test_headless_server_serves_api_without_static_console(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        with urllib.request.urlopen(f"{base_url}/api/health", timeout=5) as response:  # noqa: S310 - local test server.
            health = json.loads(response.read().decode("utf-8"))

        self.assertEqual(health["service"], "openagent-app-server")
        self.assertEqual(health["ui_enabled"], False)

        try:
            urllib.request.urlopen(f"{base_url}/", timeout=5)  # noqa: S310 - local test server.
        except urllib.error.HTTPError as error:
            self.assertEqual(error.code, 404)
            error.close()
        else:
            self.fail("headless server should not serve the static console")

    def test_server_requires_bearer_token_when_configured(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            auth_token="server-secret",
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        try:
            urllib.request.urlopen(f"{base_url}/api/health", timeout=5)  # noqa: S310 - local test server.
        except urllib.error.HTTPError as error:
            self.assertEqual(error.code, 401)
            error.close()
        else:
            self.fail("server should reject unauthenticated API requests")

        request = urllib.request.Request(f"{base_url}/api/health", headers={"Authorization": "Bearer server-secret"})
        with urllib.request.urlopen(request, timeout=5) as response:  # noqa: S310 - local test server.
            health = json.loads(response.read().decode("utf-8"))

        self.assertEqual(health["auth_required"], True)

    def test_global_event_stream_replays_after_query_sequence(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=FakeRuntime(),  # type: ignore[arg-type]
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        events = _read_sse_events(f"{base_url}/api/events?last_sequence=1", count=2)

        self.assertEqual([event["id"] for event in events], ["2", "3"])
        self.assertEqual(events[0]["event"], "turn/completed")
        self.assertEqual(events[0]["data"]["global_sequence"], 2)
        self.assertEqual(events[1]["data"]["params"]["turn_id"], "turn_2")

    def test_global_event_stream_replays_after_last_event_id_header(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=FakeRuntime(),  # type: ignore[arg-type]
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        events = _read_sse_events(f"{base_url}/api/events", count=1, headers={"Last-Event-ID": "2"})

        self.assertEqual(events[0]["id"], "3")
        self.assertEqual(events[0]["data"]["global_sequence"], 3)
        self.assertEqual(events[0]["data"]["params"]["turn_id"], "turn_2")

    def test_global_event_stream_requires_bearer_token_when_configured(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=FakeRuntime(),  # type: ignore[arg-type]
            auth_token="server-secret",
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        try:
            urllib.request.urlopen(f"{base_url}/api/events", timeout=5)  # noqa: S310 - local test server.
        except urllib.error.HTTPError as error:
            self.assertEqual(error.code, 401)
            error.close()
        else:
            self.fail("server should reject unauthenticated global SSE requests")

        events = _read_sse_events(
            f"{base_url}/api/events?last_sequence=2",
            count=1,
            headers={"Authorization": "Bearer server-secret"},
        )

        self.assertEqual(events[0]["id"], "3")

    def test_server_interrupt_endpoint_calls_runtime(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=FakeRuntime(),  # type: ignore[arg-type]
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        request = urllib.request.Request(f"{base_url}/api/turns/turn_123/interrupt", data=b"{}", method="POST")
        with urllib.request.urlopen(request, timeout=5) as response:  # noqa: S310 - local test server.
            payload = json.loads(response.read().decode("utf-8"))

        self.assertEqual(payload["turn"]["id"], "turn_123")
        self.assertEqual(payload["turn"]["status"], "interrupting")
        self.assertEqual(payload["turn"]["interrupt_requested"], True)

    def test_server_approval_endpoint_calls_runtime(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=FakeRuntime(),  # type: ignore[arg-type]
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        request = urllib.request.Request(
            f"{base_url}/api/turns/turn_123/approvals/approval_456",
            data=b'{"action":"allow","scope":"always","note":"trusted path"}',
            method="POST",
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=5) as response:  # noqa: S310 - local test server.
            payload = json.loads(response.read().decode("utf-8"))

        self.assertEqual(payload["event"]["method"], "turn/approval_resolved")
        self.assertEqual(payload["event"]["params"]["turn_id"], "turn_123")
        self.assertEqual(payload["event"]["params"]["approval"]["request_id"], "approval_456")
        self.assertEqual(payload["event"]["params"]["approval"]["action"], "allow")
        self.assertEqual(payload["event"]["params"]["approval"]["scope"], "always")
        self.assertEqual(payload["event"]["params"]["approval"]["note"], "trusted path")

    def test_server_patch_revert_endpoint_calls_runtime(self) -> None:
        workspace = self._make_temp_dir()
        runtime = FakeRuntime()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=runtime,  # type: ignore[arg-type]
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        request = urllib.request.Request(
            f"{base_url}/api/turns/turn_123/patches/last/revert",
            data=b'{"target":"a.txt"}',
            method="POST",
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=5) as response:  # noqa: S310 - local test server.
            payload = json.loads(response.read().decode("utf-8"))

        self.assertEqual(runtime.reverts, [("turn_123", "last", "a.txt")])
        self.assertEqual(payload["event"]["method"], "item/patch/reverted")
        self.assertEqual(payload["event"]["params"]["reverted"], ["a.txt: restored"])

    def test_server_session_manager_endpoints_call_runtime(self) -> None:
        workspace = self._make_temp_dir()
        runtime = FakeRuntime()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=runtime,  # type: ignore[arg-type]
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        renamed = _post_json(f"{base_url}/api/sessions/session_existing/rename", {"title": "Main"})
        forked = _post_json(f"{base_url}/api/sessions/session_existing/fork", {"title": "Branch"})
        archived = _post_json(f"{base_url}/api/sessions/session_existing/archive", {})

        self.assertEqual(renamed["session"]["title"], "Main")
        self.assertEqual(forked["session"]["id"], "session_fork")
        self.assertEqual(forked["session"]["forked_from"], "session_existing")
        self.assertEqual(archived["session"]["archived"], True)

    def test_tui_routes_require_bearer_token_when_configured(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=FakeRuntime(),  # type: ignore[arg-type]
            auth_token="server-secret",
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        try:
            _post_json(f"{base_url}/tui/append-prompt", {"text": "hello"})
        except urllib.error.HTTPError as error:
            self.assertEqual(error.code, 401)
            error.close()
        else:
            self.fail("server should reject unauthenticated TUI control requests")

        for path, method in [("/tui/control/next?timeout=0", "GET"), ("/tui/control/response", "POST")]:
            try:
                if method == "GET":
                    _get_json(f"{base_url}{path}")
                else:
                    _post_json(f"{base_url}{path}", {"ok": True})
            except urllib.error.HTTPError as error:
                self.assertEqual(error.code, 401)
                error.close()
            else:
                self.fail(f"server should reject unauthenticated {path}")

        payload = _post_json(
            f"{base_url}/tui/append-prompt",
            {"text": "hello"},
            headers={"Authorization": "Bearer server-secret"},
        )

        self.assertEqual(payload["ok"], True)
        self.assertEqual(payload["request"], {"path": "/tui/append-prompt", "body": {"text": "hello"}})

    def test_tui_control_routes_validate_body_and_queue_next_shape(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=FakeRuntime(),  # type: ignore[arg-type]
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        try:
            _post_json(f"{base_url}/tui/append-prompt", {})
        except urllib.error.HTTPError as error:
            self.assertEqual(error.code, 400)
            error.close()
        else:
            self.fail("append-prompt should require text")

        _post_json(f"{base_url}/tui/append-prompt", {"text": "hello"})
        queued = _get_json(f"{base_url}/tui/control/next?timeout=0")
        empty = _get_json(f"{base_url}/tui/control/next?timeout=0")

        self.assertEqual(queued, {"path": "/tui/append-prompt", "body": {"text": "hello"}})
        self.assertEqual(empty, {"path": "", "body": None})

    def test_tui_control_routes_map_actions_and_record_response(self) -> None:
        workspace = self._make_temp_dir()
        server = create_server(
            host="127.0.0.1",
            port=0,
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            serve_static=False,
            runtime=FakeRuntime(),  # type: ignore[arg-type]
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        base_url = f"http://{server.server_address[0]}:{server.server_address[1]}"
        cases = [
            ("/tui/submit-prompt", {}, {}),
            ("/tui/clear-prompt", {}, {}),
            ("/tui/open-help", {}, {}),
            ("/tui/open-sessions", {}, {}),
            ("/tui/open-themes", {}, {}),
            ("/tui/open-models", {}, {}),
            ("/tui/open-agents", {}, {}),
            ("/tui/open-variants", {}, {}),
            ("/tui/select-model", {"modelID": "gpt-test", "providerID": "openai"}, {"modelID": "gpt-test", "providerID": "openai"}),
            ("/tui/select-agent", {"agent": "plan"}, {"agent": "plan"}),
            ("/tui/select-variant", {"variant": "fast"}, {"variant": "fast"}),
            ("/tui/execute-command", {"command": "status"}, {"command": "status"}),
            ("/tui/show-toast", {"title": "Hi", "message": "Saved", "variant": "success", "duration": 1.5}, {"title": "Hi", "message": "Saved", "variant": "success", "duration": 1.5}),
            ("/tui/publish", {"type": "tui.model.select", "properties": {"modelID": "gpt-test"}}, {"type": "tui.model.select", "properties": {"modelID": "gpt-test"}}),
            ("/tui/publish", {"type": "tui.command.execute", "properties": {"command": "help"}}, {"type": "tui.command.execute", "properties": {"command": "help"}}),
            ("/tui/select-session", {"sessionID": "session_existing"}, {"sessionID": "session_existing"}),
        ]

        for path, body, expected_body in cases:
            _post_json(f"{base_url}{path}", body)
            queued = _get_json(f"{base_url}/tui/control/next?timeout=0")
            self.assertEqual(queued, {"path": path, "body": expected_body})

        for path, body in [
            ("/tui/execute-command", {}),
            ("/tui/select-model", {}),
            ("/tui/select-agent", {}),
            ("/tui/select-variant", {}),
            ("/tui/show-toast", {}),
            ("/tui/publish", {"type": "tui.unknown", "properties": {}}),
        ]:
            try:
                _post_json(f"{base_url}{path}", body)
            except urllib.error.HTTPError as error:
                self.assertEqual(error.code, 400)
                error.close()
            else:
                self.fail(f"{path} should reject invalid body")

        try:
            _post_json(f"{base_url}/tui/select-session", {"sessionID": "missing"})
        except urllib.error.HTTPError as error:
            self.assertEqual(error.code, 404)
            error.close()
        else:
            self.fail("select-session should reject unknown sessions when runtime can check")

        response = _post_json(f"{base_url}/tui/control/response", ["ok", {"applied": True}])
        self.assertEqual(response["ok"], True)
        self.assertEqual(response["response"], ["ok", {"applied": True}])


def _read_sse_events(url: str, *, count: int, headers: dict[str, str] | None = None) -> list[dict[str, object]]:
    request_headers = {"Connection": "close", **(headers or {})}
    request = urllib.request.Request(url, headers=request_headers)
    events: list[dict[str, object]] = []
    event_id = ""
    event_name = ""
    data_lines: list[str] = []
    with urllib.request.urlopen(request, timeout=5) as response:  # noqa: S310 - local test server.
        while len(events) < count:
            raw_line = response.readline()
            if not raw_line:
                break
            line = raw_line.decode("utf-8").rstrip("\r\n")
            if not line:
                if data_lines:
                    events.append(
                        {
                            "id": event_id,
                            "event": event_name,
                            "data": json.loads("\n".join(data_lines)),
                        }
                    )
                    event_id = ""
                    event_name = ""
                    data_lines = []
                continue
            if line.startswith(":"):
                continue
            if line.startswith("id:"):
                event_id = line.removeprefix("id:").strip()
            elif line.startswith("event:"):
                event_name = line.removeprefix("event:").strip()
            elif line.startswith("data:"):
                data_lines.append(line.removeprefix("data:").lstrip())
    return events


def _get_json(url: str, *, headers: dict[str, str] | None = None) -> dict[str, object]:
    request = urllib.request.Request(url, headers=headers or {})
    with urllib.request.urlopen(request, timeout=5) as response:  # noqa: S310 - local test server.
        value = json.loads(response.read().decode("utf-8"))
    return value if isinstance(value, dict) else {}


def _post_json(url: str, payload: object, *, headers: dict[str, str] | None = None) -> dict[str, object]:
    data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    request = urllib.request.Request(url, data=data, method="POST", headers={"Content-Type": "application/json", **(headers or {})})
    with urllib.request.urlopen(request, timeout=5) as response:  # noqa: S310 - local test server.
        value = json.loads(response.read().decode("utf-8"))
    return value if isinstance(value, dict) else {}


if __name__ == "__main__":
    unittest.main()
