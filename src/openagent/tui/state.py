from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from openagent.app_server.runtime import OpenAgentAppRuntime, TurnRecord
from openagent.cli.custom_commands import discover_commands, render_command, resolve_command

from .formatting import TimelineLine, format_event

BUILTIN_COMMANDS: tuple[tuple[str, str], ...] = (
    ("/help", "show TUI commands"),
    ("/sessions", "list recent sessions"),
    ("/resume <id>", "resume a session by id or unique prefix"),
    ("/new", "start a new session"),
    ("/clear", "clear the visible timeline"),
    ("/status", "show current session, turn, and model status"),
    ("/commands", "list project/global custom commands"),
)


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
            self._show_help()
            return "", raw_text, True
        name, *arguments = command_line.split()
        if self._handle_builtin_command(name, arguments):
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

    def _handle_builtin_command(self, name: str, arguments: list[str]) -> bool:
        if name in {"help", "?"}:
            self._show_help()
            return True
        if name == "commands":
            self._show_commands()
            return True
        if name in {"sessions", "session"}:
            self._show_sessions()
            return True
        if name in {"resume", "continue"}:
            self._resume_from_command(arguments)
            return True
        if name == "new":
            session_id = self.new_session()
            self.timeline.append(TimelineLine("status", f"new session: {session_id}", important=True))
            return True
        if name == "clear":
            self.clear()
            return True
        if name == "status":
            self._show_status()
            return True
        return False

    def _show_help(self) -> None:
        lines = [f"{name} - {description}" for name, description in BUILTIN_COMMANDS]
        self.timeline.append(TimelineLine("status", "built-in commands:\n" + "\n".join(lines), True))
        self.status = "help listed"

    def _show_sessions(self) -> None:
        try:
            sessions = list(self.runtime.list_sessions())
        except Exception as error:  # noqa: BLE001 - keep TUI failures visible.
            self.timeline.append(TimelineLine("error", f"failed to list sessions: {error}", important=True))
            self.status = "sessions failed"
            return
        if not sessions:
            self.timeline.append(TimelineLine("status", "no sessions found", important=True))
            self.status = "no sessions"
            return
        lines: list[str] = []
        for session in sessions[:20]:
            sid = str(session.get("id") or "-")
            marker = "*" if sid == self.session_id else " "
            status = str(session.get("status") or "-")
            message_count = session.get("message_count") or 0
            lines.append(f"{marker} {sid}  {status}  {message_count} msg")
        self.timeline.append(TimelineLine("status", "sessions:\n" + "\n".join(lines), important=True))
        self.status = "sessions listed"

    def _resume_from_command(self, arguments: list[str]) -> None:
        if not arguments:
            self.timeline.append(TimelineLine("warning", "usage: /resume <session-id-or-prefix>", important=True))
            self._show_sessions()
            return
        query = arguments[0]
        match = self._resolve_session_id(query)
        if match is None:
            self.timeline.append(TimelineLine("error", f"session not found: {query}", important=True))
            self.status = "session not found"
            return
        if isinstance(match, list):
            self.timeline.append(
                TimelineLine(
                    "error",
                    "session prefix is ambiguous:\n" + "\n".join(match[:10]),
                    important=True,
                )
            )
            self.status = "session ambiguous"
            return
        try:
            session = self.runtime.resume_session(match)
        except Exception as error:  # noqa: BLE001
            self.timeline.append(TimelineLine("error", f"failed to resume session {match}: {error}", important=True))
            self.status = "resume failed"
            return
        self.session_id = str(session.get("id") or match)
        self.active_turn = None
        self.next_event_index = 0
        self.input_buffer = ""
        self.timeline.clear()
        self._load_session_messages(self.session_id)
        self.timeline.append(TimelineLine("status", f"resumed session: {self.session_id}", important=True))
        self.status = "session resumed"

    def _resolve_session_id(self, query: str) -> str | list[str] | None:
        try:
            sessions = list(self.runtime.list_sessions())
        except Exception:
            return query
        ids = [str(session.get("id") or "") for session in sessions if session.get("id")]
        if query in ids:
            return query
        matches = [sid for sid in ids if sid.startswith(query)]
        if len(matches) == 1:
            return matches[0]
        if len(matches) > 1:
            return matches
        return None

    def _load_session_messages(self, session_id: str) -> None:
        get_session = getattr(self.runtime, "get_session", None)
        if not callable(get_session):
            return
        try:
            payload = get_session(session_id)
        except Exception:  # noqa: BLE001 - resume should still work without transcript rendering.
            return
        messages = payload.get("messages") if isinstance(payload, dict) else None
        if not isinstance(messages, list):
            return
        for message in messages[-30:]:
            if not isinstance(message, dict):
                continue
            role = str(message.get("role") or "message")
            content = str(message.get("content") or "").strip()
            if not content:
                continue
            if role == "user":
                self.timeline.append(TimelineLine("user", f"> {content}", important=True))
            elif role == "assistant":
                self.timeline.append(TimelineLine("assistant", content, important=False))
            elif role == "tool":
                self.timeline.append(TimelineLine("tool", f"tool result: {content}", important=False))
            else:
                self.timeline.append(TimelineLine("event", f"{role}: {content}", important=False))

    def _show_status(self) -> None:
        turn = self.active_turn
        lines = [
            f"session: {self.session_id or '-'}",
            f"turn: {getattr(turn, 'id', '-') if turn is not None else '-'}",
            f"turn_status: {turn.status if turn is not None else '-'}",
            f"events: {len(turn.events) if turn is not None else 0}",
        ]
        workspace = getattr(self.runtime, "workspace", None)
        if workspace is not None:
            lines.append(f"workspace: {workspace}")
        self.timeline.append(TimelineLine("status", "status:\n" + "\n".join(lines), important=True))
        self.status = "status shown"

    def _show_commands(self) -> None:
        commands = discover_commands(workspace=self._workspace())
        builtin_lines = [f"{name} - {description}" for name, description in BUILTIN_COMMANDS]
        if not commands:
            self.timeline.append(
                TimelineLine(
                    "status",
                    "built-in commands:\n"
                    + "\n".join(builtin_lines)
                    + "\n\nno custom commands found in .openagent/commands or ~/.config/openagent/commands",
                    True,
                )
            )
            self.status = "commands listed"
            return
        lines = ["/" + command.name + (f" - {command.description}" if command.description else "") for command in commands]
        self.timeline.append(
            TimelineLine(
                "status",
                "built-in commands:\n" + "\n".join(builtin_lines) + "\n\ncustom commands:\n" + "\n".join(lines),
                True,
            )
        )
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
