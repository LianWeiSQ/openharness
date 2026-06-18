from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from openagent.app_server.runtime import OpenAgentAppRuntime, TurnRecord
from openagent.cli.custom_commands import discover_commands, inject_file_references, render_command, resolve_command

from .formatting import TimelineLine, format_event

DEFAULT_TRANSCRIPT_LIMIT = 30
MAX_TRANSCRIPT_LIMIT = 100

BUILTIN_COMMANDS: tuple[tuple[str, str], ...] = (
    ("/help", "show TUI commands"),
    ("/sessions", "open recent session picker"),
    ("/resume <id>", "resume a session by id or unique prefix"),
    ("/transcript [limit]", "show recent messages from the current session"),
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
    session_picker_open: bool = False
    session_picker_index: int = 0
    session_picker_sessions: list[dict[str, object]] = field(default_factory=list)
    active_approval: dict[str, Any] | None = None
    file_picker_open: bool = False
    file_picker_index: int = 0
    file_picker_query: str = ""
    file_picker_matches: list[str] = field(default_factory=list)

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
            text = raw_text[1:] if raw_text.startswith("//") else raw_text
            return inject_file_references(text, workspace=self._workspace()), raw_text, False
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
            self.open_session_picker(announce=True)
            return True
        if name in {"resume", "continue"}:
            self._resume_from_command(arguments)
            return True
        if name == "transcript":
            self._transcript_from_command(arguments)
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

    def open_session_picker(self, *, announce: bool = False) -> bool:
        try:
            sessions = list(self.runtime.list_sessions())
        except Exception as error:  # noqa: BLE001 - keep picker failures visible.
            self.timeline.append(TimelineLine("error", f"failed to open session picker: {error}", important=True))
            self.status = "session picker failed"
            return False
        self.session_picker_sessions = sessions[:50]
        if not self.session_picker_sessions:
            self.session_picker_open = False
            self.session_picker_index = 0
            self.timeline.append(TimelineLine("status", "no sessions found", important=True))
            self.status = "no sessions"
            return False
        self.session_picker_index = self._current_session_picker_index()
        self.session_picker_open = True
        self.status = "session picker"
        if announce:
            self.timeline.append(
                TimelineLine(
                    "status",
                    "session picker opened. Use Up/Down or j/k, Enter to resume, Esc to close.",
                    important=True,
                )
            )
        return True

    def close_session_picker(self) -> None:
        self.session_picker_open = False
        self.status = "session picker closed"

    def move_session_picker(self, delta: int) -> None:
        if not self.session_picker_open:
            return
        if not self.session_picker_sessions:
            self.open_session_picker()
            return
        count = len(self.session_picker_sessions)
        self.session_picker_index = max(0, min(count - 1, self.session_picker_index + delta))
        selected = self.selected_session()
        if selected is not None:
            self.status = f"selected {selected.get('id') or '-'}"

    def selected_session(self) -> dict[str, object] | None:
        if not self.session_picker_sessions:
            return None
        index = max(0, min(len(self.session_picker_sessions) - 1, self.session_picker_index))
        return self.session_picker_sessions[index]

    def select_session_from_picker(self) -> bool:
        if not self.session_picker_open:
            return False
        selected = self.selected_session()
        if selected is None:
            self.status = "no session selected"
            return False
        session_id = str(selected.get("id") or "")
        if not session_id:
            self.status = "invalid session"
            return False
        self._resume_session_id(session_id)
        self.session_picker_open = False
        return True

    def refresh_file_picker(self) -> None:
        span = self._active_file_mention_span()
        if span is None:
            self.close_file_picker(update_status=False)
            return
        _, _, query = span
        if query != self.file_picker_query:
            self.file_picker_index = 0
        self.file_picker_query = query
        self.file_picker_matches = self._search_file_mentions(query)
        self.file_picker_open = bool(self.file_picker_matches)
        if self.file_picker_index >= len(self.file_picker_matches):
            self.file_picker_index = max(0, len(self.file_picker_matches) - 1)
        if self.file_picker_open:
            self.status = "file picker"

    def close_file_picker(self, *, update_status: bool = True) -> None:
        self.file_picker_open = False
        self.file_picker_index = 0
        self.file_picker_query = ""
        self.file_picker_matches = []
        if update_status:
            self.status = "file picker closed"

    def move_file_picker(self, delta: int) -> None:
        if not self.file_picker_open:
            return
        count = len(self.file_picker_matches)
        if count == 0:
            self.close_file_picker()
            return
        self.file_picker_index = max(0, min(count - 1, self.file_picker_index + delta))
        self.status = f"selected @{self.file_picker_matches[self.file_picker_index]}"

    def selected_file_mention(self) -> str | None:
        if not self.file_picker_matches:
            return None
        index = max(0, min(len(self.file_picker_matches) - 1, self.file_picker_index))
        return self.file_picker_matches[index]

    def select_file_mention(self) -> bool:
        selected = self.selected_file_mention()
        span = self._active_file_mention_span()
        if selected is None or span is None:
            self.close_file_picker()
            return False
        start, end, _query = span
        self.input_buffer = self.input_buffer[:start] + "@" + selected + " " + self.input_buffer[end:]
        self.close_file_picker(update_status=False)
        self.status = f"inserted @{selected}"
        return True

    def _active_file_mention_span(self) -> tuple[int, int, str] | None:
        match = re.search(r"(?<!\S)@([A-Za-z0-9_./~+-]*)$", self.input_buffer)
        if not match:
            return None
        return match.start(), match.end(), match.group(1)

    def _search_file_mentions(self, query: str, *, limit: int = 30) -> list[str]:
        workspace = self._workspace()
        query_lower = query.lower()
        skip_dirs = {
            ".git",
            ".hg",
            ".svn",
            ".openagent",
            ".mypy_cache",
            ".pytest_cache",
            ".ruff_cache",
            ".venv",
            "__pycache__",
            "build",
            "dist",
            "node_modules",
        }
        matches: list[str] = []
        visited = 0
        for root, dirs, files in os.walk(workspace):
            dirs[:] = [name for name in dirs if name not in skip_dirs and not name.startswith(".tox")]
            for filename in files:
                if filename.startswith(".DS_Store"):
                    continue
                path = Path(root) / filename
                try:
                    rel = path.relative_to(workspace).as_posix()
                except ValueError:
                    continue
                visited += 1
                if visited > 5000:
                    break
                rel_lower = rel.lower()
                name_lower = filename.lower()
                if not query_lower or query_lower in rel_lower or name_lower.startswith(query_lower):
                    matches.append(rel)
            if visited > 5000:
                break
        return sorted(matches, key=lambda item: _file_match_score(item, query_lower))[:limit]

    def _current_session_picker_index(self) -> int:
        if self.session_id:
            for index, session in enumerate(self.session_picker_sessions):
                if str(session.get("id") or "") == self.session_id:
                    return index
        return 0

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
        self._resume_session_id(match)

    def _transcript_from_command(self, arguments: list[str]) -> None:
        limit = self._parse_transcript_limit(arguments)
        if limit is None:
            return
        if not self.session_id:
            self.timeline.append(TimelineLine("error", "no active session for transcript", important=True))
            self.status = "no session"
            return
        self._append_session_messages(self.session_id, limit=limit, announce=True, report_errors=True)

    def _parse_transcript_limit(self, arguments: list[str]) -> int | None:
        if not arguments:
            return DEFAULT_TRANSCRIPT_LIMIT
        if len(arguments) > 1:
            self.timeline.append(TimelineLine("warning", "usage: /transcript [limit]", important=True))
            self.status = "transcript invalid"
            return None
        raw_limit = arguments[0]
        try:
            limit = int(raw_limit)
        except ValueError:
            self.timeline.append(TimelineLine("error", f"invalid transcript limit: {raw_limit}", important=True))
            self.status = "transcript invalid"
            return None
        if limit < 1 or limit > MAX_TRANSCRIPT_LIMIT:
            self.timeline.append(
                TimelineLine(
                    "error",
                    f"transcript limit must be between 1 and {MAX_TRANSCRIPT_LIMIT}",
                    important=True,
                )
            )
            self.status = "transcript invalid"
            return None
        return limit

    def _resume_session_id(self, session_id: str) -> None:
        try:
            session = self.runtime.resume_session(session_id)
        except Exception as error:  # noqa: BLE001
            self.timeline.append(TimelineLine("error", f"failed to resume session {session_id}: {error}", important=True))
            self.status = "resume failed"
            return
        self.session_id = str(session.get("id") or session_id)
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
        self._append_session_messages(session_id, limit=DEFAULT_TRANSCRIPT_LIMIT, announce=False, report_errors=False)

    def _append_session_messages(
        self,
        session_id: str,
        *,
        limit: int,
        announce: bool,
        report_errors: bool,
    ) -> None:
        get_session = getattr(self.runtime, "get_session", None)
        if not callable(get_session):
            if report_errors:
                self.timeline.append(TimelineLine("warning", "transcript is not supported by this runtime", important=True))
                self.status = "transcript unsupported"
            return
        try:
            payload = get_session(session_id)
        except Exception as error:  # noqa: BLE001 - transcript failures should be visible when requested.
            if report_errors:
                self.timeline.append(TimelineLine("error", f"failed to load transcript: {error}", important=True))
                self.status = "transcript failed"
            return
        messages = payload.get("messages") if isinstance(payload, dict) else None
        if not isinstance(messages, list):
            if report_errors:
                self.timeline.append(TimelineLine("error", "session transcript is unavailable", important=True))
                self.status = "transcript unavailable"
            return
        lines = self._session_message_lines(messages[-limit:])
        if not lines:
            if report_errors:
                self.timeline.append(TimelineLine("status", f"transcript is empty for session: {session_id}", important=True))
                self.status = "transcript empty"
            return
        if announce:
            shown_count = min(limit, len(messages))
            self.timeline.append(
                TimelineLine(
                    "status",
                    f"transcript: {session_id} (last {shown_count} of {len(messages)} messages)",
                    important=True,
                )
            )
        self.timeline.extend(lines)
        if report_errors:
            self.status = "transcript shown"

    def _session_message_lines(self, messages: list[object]) -> list[TimelineLine]:
        lines: list[TimelineLine] = []
        for message in messages:
            if not isinstance(message, dict):
                continue
            role = str(message.get("role") or "message")
            content = str(message.get("content") or "").strip()
            if not content:
                continue
            if role == "user":
                lines.append(TimelineLine("user", f"> {content}", important=True))
            elif role == "assistant":
                lines.append(TimelineLine("assistant", content, important=False))
            elif role == "tool":
                lines.append(TimelineLine("tool", f"tool result: {content}", important=False))
            else:
                lines.append(TimelineLine("event", f"{role}: {content}", important=False))
        return lines

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
        self.active_approval = None
        self.close_file_picker(update_status=False)
        self.timeline.clear()
        self.status = "new session"
        return self.session_id

    def clear(self) -> None:
        self.timeline.clear()
        self.scroll = 0
        self.close_file_picker(update_status=False)
        self.status = "cleared"

    def poll_events(self) -> None:
        turn = self.active_turn
        if turn is None:
            return
        while self.next_event_index < len(turn.events):
            event = turn.events[self.next_event_index]
            self.timeline.extend(format_event(event))
            self._apply_control_event(event)
            self.next_event_index += 1
        if turn.status in {"completed", "failed", "interrupted"}:
            self.active_approval = None
        self.status = "approval required" if self.active_approval is not None else turn.status

    def _apply_control_event(self, event: AppEvent) -> None:
        if event.method == "turn/approval_requested":
            approval = event.params.get("approval")
            if isinstance(approval, dict):
                self.active_approval = dict(approval)
            return
        if event.method == "turn/approval_resolved":
            approval = event.params.get("approval")
            if not isinstance(approval, dict):
                self.active_approval = None
                return
            request_id = str(approval.get("request_id") or "")
            active_id = str((self.active_approval or {}).get("request_id") or "")
            if not request_id or request_id == active_id:
                self.active_approval = None

    def request_interrupt(self) -> None:
        turn = self.active_turn
        if turn is None:
            self.status = "no active turn"
            return
        interrupt_turn = getattr(self.runtime, "interrupt_turn", None)
        if not callable(interrupt_turn):
            self.timeline.append(TimelineLine("warning", "interrupt is not supported by this runtime", important=True))
            self.status = "interrupt unsupported"
            return
        interrupt_turn(turn.id)
        self.status = "interrupting"

    def respond_approval(self, action: str) -> bool:
        approval = self.active_approval
        if approval is None:
            self.status = "no approval"
            return False
        request_id = str(approval.get("request_id") or "")
        turn_id = str(approval.get("turn_id") or getattr(self.active_turn, "id", "") or "")
        if not request_id or not turn_id:
            self.timeline.append(TimelineLine("error", "approval request is missing an id", important=True))
            self.status = "approval invalid"
            return False
        respond_approval = getattr(self.runtime, "respond_approval", None)
        if not callable(respond_approval):
            self.timeline.append(TimelineLine("warning", "approval is not supported by this runtime", important=True))
            self.status = "approval unsupported"
            return False
        try:
            respond_approval(turn_id, request_id, action)
        except Exception as error:  # noqa: BLE001 - approval failures should stay visible in the TUI.
            self.timeline.append(TimelineLine("error", f"approval failed: {error}", important=True))
            self.status = "approval failed"
            return False
        self.active_approval = None
        self.status = f"approval {action} sent"
        return True


def _file_match_score(path: str, query: str) -> tuple[int, int, str]:
    lowered = path.lower()
    name = Path(path).name.lower()
    if query and lowered.startswith(query):
        rank = 0
    elif query and name.startswith(query):
        rank = 1
    elif query and query in lowered:
        rank = 2
    else:
        rank = 3
    return rank, len(path), path
