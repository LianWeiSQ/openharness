from __future__ import annotations

from dataclasses import dataclass, field

from openagent.app_server.runtime import OpenAgentAppRuntime, TurnRecord

from .formatting import TimelineLine, format_event


@dataclass(slots=True)
class TuiState:
    runtime: OpenAgentAppRuntime
    session_id: str | None = None
    active_turn: TurnRecord | None = None
    next_event_index: int = 0
    timeline: list[TimelineLine] = field(default_factory=list)
    input_buffer: str = ""
    status: str = "idle"
    scroll: int = 0

    def ensure_session(self) -> str:
        if self.session_id:
            return self.session_id
        session = self.runtime.start_session()
        self.session_id = str(session["id"])
        self.status = "session ready"
        return self.session_id

    @property
    def is_running(self) -> bool:
        return self.active_turn is not None and self.active_turn.status not in {"completed", "failed", "interrupted"}

    def submit(self) -> bool:
        text = self.input_buffer.strip()
        if not text or self.is_running:
            return False
        session_id = self.ensure_session()
        self.active_turn = self.runtime.start_turn(session_id=session_id, user_text=text)
        self.next_event_index = 0
        self.input_buffer = ""
        self.status = "running"
        self.timeline.append(TimelineLine("user", f"> {text}", important=True))
        return True

    def new_session(self) -> str:
        session = self.runtime.start_session()
        self.session_id = str(session["id"])
        self.active_turn = None
        self.next_event_index = 0
        self.input_buffer = ""
        self.timeline.clear()
        self.status = "new session"
        return self.session_id

    def clear(self) -> None:
        self.timeline.clear()
        self.scroll = 0
        self.status = "cleared"

    def poll_events(self) -> None:
        turn = self.active_turn
        if turn is None:
            return
        while self.next_event_index < len(turn.events):
            event = turn.events[self.next_event_index]
            self.timeline.extend(format_event(event))
            self.next_event_index += 1
        self.status = turn.status

    def request_interrupt(self) -> None:
        self.timeline.append(
            TimelineLine(
                "warning",
                "interrupt requested, but cooperative cancellation is not implemented yet",
                important=True,
            )
        )
        self.status = "interrupt unsupported"
