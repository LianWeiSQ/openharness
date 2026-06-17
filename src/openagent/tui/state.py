from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from openagent.app_server.runtime import OpenAgentAppRuntime, TurnRecord
from openagent.cli.custom_commands import discover_commands, render_command, resolve_command

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
        raw_text = self.input_buffer.strip()
        text, display_text, handled = self._prepare_submission(raw_text)
        if handled:
            self.input_buffer = ""
            return False
        if not text or self.is_running:
            return False
        session_id = self.ensure_session()
        self.active_turn = self.runtime.start_turn(session_id=session_id, user_text=text)
        self.next_event_index = 0
        self.input_buffer = ""
        self.status = "running"
        self.timeline.append(TimelineLine("user", f"> {display_text}", important=True))
        return True

    def _prepare_submission(self, raw_text: str) -> tuple[str, str, bool]:
        if not raw_text or not raw_text.startswith("/") or raw_text.startswith("//"):
            return raw_text[1:] if raw_text.startswith("//") else raw_text, raw_text, False
        command_line = raw_text[1:].strip()
        if not command_line:
            self._show_commands()
            return "", raw_text, True
        name, *arguments = command_line.split()
        if name in {"help", "commands"}:
            self._show_commands()
            return "", raw_text, True
        try:
            command = resolve_command(name, workspace=self._workspace())
            rendered = render_command(command, arguments, workspace=self._workspace())
        except FileNotFoundError:
            self.timeline.append(TimelineLine("error", f"slash command not found: /{name}", important=True))
            self.status = "command not found"
            return "", raw_text, True
        except Exception as error:  # noqa: BLE001 - command rendering errors should be visible in the TUI.
            self.timeline.append(TimelineLine("error", f"slash command failed: /{name}\n{error}", important=True))
            self.status = "command failed"
            return "", raw_text, True
        self.timeline.append(TimelineLine("status", f"slash command: /{name}", important=True))
        return rendered, raw_text, False

    def _show_commands(self) -> None:
        commands = discover_commands(workspace=self._workspace())
        if not commands:
            self.timeline.append(TimelineLine("status", "no custom commands found in .openagent/commands or ~/.config/openagent/commands", True))
            self.status = "no commands"
            return
        lines = ["/" + command.name + (f" - {command.description}" if command.description else "") for command in commands]
        self.timeline.append(TimelineLine("status", "custom commands:\n" + "\n".join(lines), True))
        self.status = "commands listed"

    def _workspace(self) -> Path:
        workspace = getattr(self.runtime, "workspace", None)
        if workspace is None:
            return Path.cwd()
        return Path(workspace).expanduser().resolve()

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
