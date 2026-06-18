from __future__ import annotations

import threading
import time
import urllib.error
import urllib.request
from collections.abc import Iterator
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from openagent.app_server.protocol import AppEvent
from openagent.cli.main import (
    AppBridgeClientError,
    app_bridge_get_json,
    app_bridge_post_json,
    format_http_error,
    join_server_url,
    normalize_server_url,
    parse_sse_response,
    quote_path,
    stream_app_bridge_events,
)


TERMINAL_METHODS = {"turn/completed", "turn/failed", "turn/interrupted"}
TERMINAL_STATUSES = {"completed", "failed", "interrupted"}


@dataclass(slots=True)
class RemoteTurnRecord:
    id: str
    session_id: str
    status: str = "queued"
    final_answer: str = ""
    error: str | None = None
    trace: dict[str, Any] | None = None
    events: list[AppEvent] = field(default_factory=list)
    _seen_event_keys: set[tuple[object, ...]] = field(default_factory=set, repr=False)
    _condition: threading.Condition = field(default_factory=threading.Condition, repr=False)

    @classmethod
    def from_payload(cls, payload: dict[str, object], *, session_id: str) -> "RemoteTurnRecord":
        return cls(
            id=str(payload.get("id") or ""),
            session_id=str(payload.get("session_id") or session_id),
            status=str(payload.get("status") or "queued"),
            final_answer=str(payload.get("final_answer") or ""),
            error=str(payload.get("error") or "") or None,
            trace=dict(payload.get("trace")) if isinstance(payload.get("trace"), dict) else None,
        )

    def append_event(self, event: AppEvent) -> bool:
        with self._condition:
            key = _remote_event_key(event, default_turn_id=self.id)
            if key in self._seen_event_keys:
                return False
            self._seen_event_keys.add(key)
            self.events.append(event)
            self._apply_event_locked(event)
            self._condition.notify_all()
            return True

    def mark_failed(self, error: str) -> None:
        with self._condition:
            self.status = "failed"
            self.error = error
            self._condition.notify_all()

    def wait_for_sequence(self, sequence: int, *, timeout_s: float = 15.0) -> AppEvent | None:
        deadline = time.time() + timeout_s
        with self._condition:
            while len(self.events) < sequence and self.status not in TERMINAL_STATUSES:
                remaining = deadline - time.time()
                if remaining <= 0:
                    return None
                self._condition.wait(timeout=remaining)
            if len(self.events) >= sequence:
                return self.events[sequence - 1]
            return None

    def wait_until_terminal(self, *, timeout_s: float = 30.0) -> bool:
        deadline = time.time() + timeout_s
        with self._condition:
            while self.status not in TERMINAL_STATUSES:
                remaining = deadline - time.time()
                if remaining <= 0:
                    return False
                self._condition.wait(timeout=remaining)
            return True

    def _apply_event_locked(self, event: AppEvent) -> None:
        params = event.params
        if event.method == "turn/approval_requested":
            self.status = str(params.get("status") or "waiting_approval")
            return
        if event.method == "turn/approval_resolved":
            self.status = str(params.get("status") or "running")
            return
        if event.method == "turn/started":
            self.status = str(params.get("status") or "running")
            return
        if event.method in TERMINAL_METHODS:
            default_status = "completed" if event.method == "turn/completed" else ("interrupted" if event.method == "turn/interrupted" else "failed")
            self.status = str(params.get("status") or default_status)
            self.final_answer = str(params.get("final_answer") or self.final_answer or "")
            self.error = str(params.get("error") or self.error or "") or None
            self.trace = dict(params.get("trace")) if isinstance(params.get("trace"), dict) else self.trace


class RemoteAppBridgeRuntime:
    """Duck-typed TUI runtime backed by a running App Bridge server."""

    def __init__(
        self,
        *,
        server_url: str,
        workspace: str | Path | None = None,
        auth_token: str | None = None,
        use_global_events: bool = True,
    ) -> None:
        self.server_url = normalize_server_url(server_url)
        self.workspace = Path(workspace or Path.cwd()).expanduser().resolve()
        self.auth_token = auth_token
        self._turns: dict[str, RemoteTurnRecord] = {}
        self._turns_lock = threading.Lock()
        self._use_global_events = use_global_events
        self._global_stream_active = False
        self._global_stream_unavailable = not use_global_events
        self._global_stream_started = False
        self._global_stream_lock = threading.Lock()
        self._global_stream_stop = threading.Event()
        self._control_requests: list[dict[str, object]] = []
        self._control_condition = threading.Condition()
        self._control_poll_started = False
        self._control_poll_unavailable = False
        self._control_poll_stop = threading.Event()
        if use_global_events:
            self._ensure_global_stream()
        self._ensure_tui_control_poll()

    def start_session(self, *, cwd: str | Path | None = None) -> dict[str, object]:
        payload = app_bridge_post_json(
            self.server_url,
            "/api/sessions",
            {"cwd": str(Path(cwd or self.workspace).expanduser().resolve())},
            auth_token=self.auth_token,
        )
        return _session_from_payload(payload)

    def resume_session(self, session_id: str) -> dict[str, object]:
        return self.get_session(session_id)

    def list_sessions(self) -> list[dict[str, object]]:
        payload = app_bridge_get_json(self.server_url, "/api/sessions", auth_token=self.auth_token)
        sessions = payload.get("sessions")
        return [dict(item) for item in sessions if isinstance(item, dict)] if isinstance(sessions, list) else []

    def get_session(self, session_id: str) -> dict[str, object]:
        payload = app_bridge_get_json(self.server_url, f"/api/sessions/{quote_path(session_id)}", auth_token=self.auth_token)
        return _session_from_payload(payload)

    def start_turn(self, *, session_id: str, user_text: str) -> RemoteTurnRecord:
        payload = app_bridge_post_json(
            self.server_url,
            f"/api/sessions/{quote_path(session_id)}/turns",
            {"input": user_text},
            auth_token=self.auth_token,
        )
        raw_turn = payload.get("turn")
        if not isinstance(raw_turn, dict):
            raise ValueError("server returned an invalid turn payload")
        turn = RemoteTurnRecord.from_payload(raw_turn, session_id=session_id)
        if not turn.id:
            raise ValueError("server returned a turn without an id")
        with self._turns_lock:
            existing_turn = self._turns.get(turn.id)
            if existing_turn is None:
                self._turns[turn.id] = turn
            else:
                existing_turn.session_id = turn.session_id or existing_turn.session_id
                if existing_turn.status == "queued" or turn.status != "queued":
                    existing_turn.status = turn.status
                existing_turn.final_answer = turn.final_answer or existing_turn.final_answer
                existing_turn.error = turn.error or existing_turn.error
                existing_turn.trace = turn.trace or existing_turn.trace
                turn = existing_turn
        if self._should_start_turn_stream():
            thread = threading.Thread(target=self._consume_turn_events, args=(turn,), daemon=True)
            thread.start()
        return turn

    def get_turn(self, turn_id: str) -> RemoteTurnRecord:
        with self._turns_lock:
            turn = self._turns.get(turn_id)
        if turn is None:
            raise KeyError(f"Unknown turn: {turn_id}")
        return turn

    def interrupt_turn(self, turn_id: str) -> dict[str, object]:
        payload = app_bridge_post_json(self.server_url, f"/api/turns/{quote_path(turn_id)}/interrupt", {}, auth_token=self.auth_token)
        turn = payload.get("turn")
        return dict(turn) if isinstance(turn, dict) else payload

    def respond_approval(self, turn_id: str, request_id: str, action: str) -> dict[str, object]:
        payload = app_bridge_post_json(
            self.server_url,
            f"/api/turns/{quote_path(turn_id)}/approvals/{quote_path(request_id)}",
            {"action": action},
            auth_token=self.auth_token,
        )
        event = payload.get("event")
        return dict(event) if isinstance(event, dict) else payload

    def drain_control_requests(self) -> list[dict[str, object]]:
        with self._control_condition:
            requests = list(self._control_requests)
            self._control_requests.clear()
        return requests

    def post_control_response(
        self,
        request_id: str,
        *,
        ok: bool = True,
        result: dict[str, object] | None = None,
        error: str | None = None,
    ) -> dict[str, object]:
        payload: dict[str, object] = {"id": request_id, "ok": ok}
        if result is not None:
            payload["result"] = result
        if error:
            payload["error"] = error
        return app_bridge_post_json(self.server_url, "/tui/control/response", payload, auth_token=self.auth_token)

    def _consume_turn_events(self, turn: RemoteTurnRecord) -> None:
        try:
            for raw_event in stream_app_bridge_events(self.server_url, turn.id, auth_token=self.auth_token):
                turn.append_event(_app_event_from_dict(raw_event, default_sequence=len(turn.events) + 1))
        except Exception as error:  # noqa: BLE001 - remote failures need to surface in the TUI.
            if not self._is_global_stream_active():
                turn.mark_failed(str(error))

    def _ensure_global_stream(self) -> None:
        with self._global_stream_lock:
            if self._global_stream_started or self._global_stream_unavailable:
                return
            self._global_stream_started = True
        thread = threading.Thread(target=self._consume_global_events, daemon=True)
        thread.start()

    def _consume_global_events(self) -> None:
        last_sequence = 0
        while not self._global_stream_stop.is_set():
            try:
                for raw_event in self._stream_global_events(last_sequence=last_sequence):
                    event = _app_event_from_dict(raw_event, default_sequence=0)
                    if event.global_sequence is not None:
                        last_sequence = max(last_sequence, event.global_sequence)
                    self._route_global_event(event)
            except Exception:  # noqa: BLE001 - a missing global endpoint should fall back silently.
                with self._global_stream_lock:
                    had_active_stream = self._global_stream_active or last_sequence > 0
                    self._global_stream_active = False
                    if not had_active_stream:
                        self._global_stream_unavailable = True
                        return
                time.sleep(0.25)
                continue
            time.sleep(0.1)

    def _stream_global_events(self, *, last_sequence: int) -> Iterator[dict[str, object]]:
        headers = {"Accept": "text/event-stream"}
        if self.auth_token:
            headers["Authorization"] = f"Bearer {self.auth_token}"
        if last_sequence > 0:
            headers["Last-Event-ID"] = str(last_sequence)
        path = "/api/events" if last_sequence <= 0 else f"/api/events?last_sequence={last_sequence}"
        request = urllib.request.Request(url=join_server_url(self.server_url, path), headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=60) as response:  # noqa: S310 - user-selected local/remote App Bridge URL.
                self._mark_global_stream_active()
                yield from parse_sse_response(response)
        except urllib.error.HTTPError as error:
            raise AppBridgeClientError(format_http_error("GET", path, error)) from error
        except urllib.error.URLError as error:
            raise AppBridgeClientError(str(error.reason)) from error

    def _route_global_event(self, event: AppEvent) -> None:
        turn_id = _event_turn_id(event)
        if not turn_id:
            return
        session_id = _event_session_id(event)
        with self._turns_lock:
            turn = self._turns.get(turn_id)
            if turn is None:
                turn = RemoteTurnRecord(id=turn_id, session_id=session_id)
                self._turns[turn_id] = turn
        turn.append_event(event)

    def _mark_global_stream_active(self) -> None:
        with self._global_stream_lock:
            self._global_stream_active = True
            self._global_stream_unavailable = False

    def _is_global_stream_active(self) -> bool:
        with self._global_stream_lock:
            return self._global_stream_active

    def _should_start_turn_stream(self) -> bool:
        if not self._use_global_events:
            return True
        with self._global_stream_lock:
            return self._global_stream_unavailable or not self._global_stream_active

    def _ensure_tui_control_poll(self) -> None:
        with self._control_condition:
            if self._control_poll_started or self._control_poll_unavailable:
                return
            self._control_poll_started = True
        thread = threading.Thread(target=self._poll_tui_control, daemon=True)
        thread.start()

    def _poll_tui_control(self) -> None:
        while not self._control_poll_stop.is_set():
            try:
                payload = app_bridge_get_json(self.server_url, "/tui/control/next?timeout=0.25", auth_token=self.auth_token)
            except Exception:  # noqa: BLE001 - older or closing servers should disable remote TUI control polling.
                with self._control_condition:
                    self._control_poll_unavailable = True
                    self._control_condition.notify_all()
                return
            request = payload.get("request")
            if isinstance(request, dict):
                with self._control_condition:
                    self._control_requests.append(dict(request))
                    self._control_condition.notify_all()
            else:
                time.sleep(0.05)


def _session_from_payload(payload: dict[str, object]) -> dict[str, object]:
    session = payload.get("session")
    if isinstance(session, dict):
        return dict(session)
    raise ValueError("server returned an invalid session payload")


def _app_event_from_dict(payload: dict[str, object], *, default_sequence: int) -> AppEvent:
    params = payload.get("params")
    created_at_ms = payload.get("created_at_ms")
    global_sequence = payload.get("global_sequence")
    return AppEvent(
        sequence=int(payload.get("sequence") or default_sequence),
        method=str(payload.get("method") or ""),
        params=dict(params) if isinstance(params, dict) else {},
        created_at_ms=int(created_at_ms) if isinstance(created_at_ms, int | float) else int(time.time() * 1000),
        global_sequence=int(global_sequence) if isinstance(global_sequence, int | float) else None,
    )


def _event_turn_id(event: AppEvent) -> str:
    raw_turn_id = event.params.get("turn_id")
    if raw_turn_id:
        return str(raw_turn_id)
    approval = event.params.get("approval")
    if isinstance(approval, dict) and approval.get("turn_id"):
        return str(approval["turn_id"])
    return ""


def _event_session_id(event: AppEvent) -> str:
    return str(event.params.get("thread_id") or event.params.get("session_id") or "")


def _remote_event_key(event: AppEvent, *, default_turn_id: str) -> tuple[object, ...]:
    if event.global_sequence is not None:
        return ("global", event.global_sequence)
    return ("turn", _event_turn_id(event) or default_turn_id, event.sequence, event.method)
