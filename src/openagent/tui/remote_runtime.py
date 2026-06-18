from __future__ import annotations

import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from openagent.app_server.protocol import AppEvent
from openagent.cli.main import (
    app_bridge_get_json,
    app_bridge_post_json,
    normalize_server_url,
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

    def append_event(self, event: AppEvent) -> None:
        with self._condition:
            self.events.append(event)
            self._apply_event_locked(event)
            self._condition.notify_all()

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
    ) -> None:
        self.server_url = normalize_server_url(server_url)
        self.workspace = Path(workspace or Path.cwd()).expanduser().resolve()
        self.auth_token = auth_token
        self._turns: dict[str, RemoteTurnRecord] = {}

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
        self._turns[turn.id] = turn
        thread = threading.Thread(target=self._consume_turn_events, args=(turn,), daemon=True)
        thread.start()
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

    def _consume_turn_events(self, turn: RemoteTurnRecord) -> None:
        try:
            for raw_event in stream_app_bridge_events(self.server_url, turn.id, auth_token=self.auth_token):
                turn.append_event(_app_event_from_dict(raw_event, default_sequence=len(turn.events) + 1))
        except Exception as error:  # noqa: BLE001 - remote failures need to surface in the TUI.
            turn.mark_failed(str(error))


def _session_from_payload(payload: dict[str, object]) -> dict[str, object]:
    session = payload.get("session")
    if isinstance(session, dict):
        return dict(session)
    raise ValueError("server returned an invalid session payload")


def _app_event_from_dict(payload: dict[str, object], *, default_sequence: int) -> AppEvent:
    params = payload.get("params")
    created_at_ms = payload.get("created_at_ms")
    return AppEvent(
        sequence=int(payload.get("sequence") or default_sequence),
        method=str(payload.get("method") or ""),
        params=dict(params) if isinstance(params, dict) else {},
        created_at_ms=int(created_at_ms) if isinstance(created_at_ms, int | float) else int(time.time() * 1000),
    )
