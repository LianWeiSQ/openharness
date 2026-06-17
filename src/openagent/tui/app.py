from __future__ import annotations

import argparse
import curses
import json
import os
import textwrap
import time
from pathlib import Path
from typing import Any

from openagent.app_server.runtime import OpenAgentAppRuntime

from .formatting import TimelineLine, short_id, trace_label, wrap_lines
from .state import TuiState


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description="Run the OpenAgent terminal UI.")
    parser.add_argument("--workspace", default=None)
    parser.add_argument("--session-root", default=None)
    args = parser.parse_args(argv)

    runtime = OpenAgentAppRuntime(workspace=args.workspace, session_store_root=args.session_root)
    state = TuiState(runtime=runtime)
    try:
        curses.wrapper(lambda stdscr: _run(stdscr, state))
    except KeyboardInterrupt:
        return


def _run(stdscr, state: TuiState) -> None:
    curses.curs_set(1)
    curses.use_default_colors()
    _init_colors()
    stdscr.timeout(120)
    state.ensure_session()

    while True:
        state.poll_events()
        _render(stdscr, state)
        key = stdscr.getch()
        if key == -1:
            continue
        if _handle_key(key, state):
            break


def _handle_key(key: int, state: TuiState) -> bool:
    if state.active_approval is not None:
        if key in {ord("a"), ord("A"), ord("y"), ord("Y")}:
            state.respond_approval("allow")
            return False
        if key in {ord("d"), ord("D"), ord("n"), ord("N"), 27}:
            state.respond_approval("deny")
            return False
        if key in {3}:  # Ctrl-C
            state.respond_approval("deny")
            state.request_interrupt()
            return False
        return False
    if key in {3}:  # Ctrl-C
        if state.is_running:
            state.request_interrupt()
            return False
        return True
    if key in {4}:  # Ctrl-D
        return True
    if key == 27 and state.session_picker_open:  # Esc
        state.close_session_picker()
        return False
    if key == 27 and not state.is_running and not state.input_buffer:  # Esc
        return True
    if state.session_picker_open:
        if key in {10, 13}:
            state.select_session_from_picker()
            return False
        if key in {curses.KEY_UP, ord("k")}:
            state.move_session_picker(-1)
            return False
        if key in {curses.KEY_DOWN, ord("j")}:
            state.move_session_picker(1)
            return False
        if key == curses.KEY_PPAGE:
            state.move_session_picker(-5)
            return False
        if key == curses.KEY_NPAGE:
            state.move_session_picker(5)
            return False
    if key in {14}:  # Ctrl-N
        state.new_session()
        return False
    if key in {18}:  # Ctrl-R
        state.open_session_picker(announce=True)
        return False
    if key in {12}:  # Ctrl-L
        state.clear()
        return False
    if key in {10, 13}:
        state.submit()
        return False
    if key in {curses.KEY_BACKSPACE, 127, 8}:
        state.input_buffer = state.input_buffer[:-1]
        return False
    if key == curses.KEY_PPAGE:
        state.scroll += 8
        return False
    if key == curses.KEY_NPAGE:
        state.scroll = max(0, state.scroll - 8)
        return False
    if key == curses.KEY_RESIZE:
        return False
    if 32 <= key <= 126:
        state.input_buffer += chr(key)
    return False


def _render(stdscr, state: TuiState) -> None:
    stdscr.erase()
    height, width = stdscr.getmaxyx()
    if height < 8 or width < 40:
        _addstr(stdscr, 0, 0, "OpenAgent TUI needs at least 40x8 terminal size.", curses.color_pair(4))
        stdscr.refresh()
        return

    header_h = 3
    input_h = 4
    body_h = max(1, height - header_h - input_h - 1)
    side_w = 28 if width >= 112 else 0
    detail_w = 34 if width >= 132 else 0
    main_x = side_w
    main_w = width - side_w - detail_w

    _render_header(stdscr, state, width)
    if side_w:
        _render_sessions(stdscr, state, header_h, side_w, body_h)
    _render_timeline(stdscr, state, header_h, main_x, main_w, body_h)
    if detail_w:
        _render_details(stdscr, state, header_h, width - detail_w, detail_w, body_h)
    _render_input(stdscr, state, height - input_h - 1, 0, width, input_h)
    _render_footer(stdscr, state, height - 1, width)
    stdscr.refresh()


def _render_header(stdscr, state: TuiState, width: int) -> None:
    title = "OpenAgent TUI"
    session = short_id(state.session_id)
    model = os.getenv("OPENAI_MODEL") or "env:OPENAI_MODEL"
    status = state.status
    _addstr(stdscr, 0, 0, title, curses.color_pair(1) | curses.A_BOLD)
    _addstr(stdscr, 0, len(title) + 2, f"session {session}", curses.color_pair(2))
    right = f"{status} | {model}"
    _addstr(stdscr, 0, max(0, width - len(right) - 1), right[: max(0, width - 1)], curses.color_pair(2))
    _hline(stdscr, 2, 0, width)


def _render_sessions(stdscr, state: TuiState, y: int, width: int, height: int) -> None:
    title = "Sessions"
    if state.session_picker_open:
        title = "Sessions picker"
    _addstr(stdscr, y, 1, title, curses.color_pair(1) | curses.A_BOLD)
    sessions = _sessions_for_render(state, limit=max(0, height - 3))
    for idx, session in enumerate(sessions, start=1):
        sid = str(session.get("id") or "-")
        marker = "*" if sid == state.session_id else " "
        selected = state.session_picker_open and (idx - 1) == state.session_picker_index
        if selected:
            marker = ">"
        line = f"{marker} {short_id(sid, keep=16)}"
        attr = curses.color_pair(1) | curses.A_BOLD if selected else curses.color_pair(3 if marker == "*" else 2)
        _addstr(stdscr, y + idx, 1, line[: width - 2], attr)
        meta = f"  {session.get('message_count') or 0} msg"
        if session.get("status"):
            meta += f"  {session.get('status')}"
        _addstr(stdscr, y + idx + 1, 1, meta[: width - 2], curses.color_pair(2))
    if state.session_picker_open and height >= 4:
        hint = "Enter resume | Esc close"
        _addstr(stdscr, y + height - 1, 1, hint[: width - 2], curses.color_pair(2))
    _vline(stdscr, y, width - 1, height)


def _sessions_for_render(state: TuiState, *, limit: int) -> list[dict[str, object]]:
    if state.session_picker_open:
        return state.session_picker_sessions[:limit]
    try:
        return list(state.runtime.list_sessions())[:limit]
    except Exception as error:  # noqa: BLE001
        return [{"id": "error", "status": str(error), "message_count": 0}]


def _render_timeline(stdscr, state: TuiState, y: int, x: int, width: int, height: int) -> None:
    inner_w = max(10, width - 3)
    lines = wrap_lines(state.timeline or [TimelineLine("event", "No events yet. Type a task and press Enter.")], width=inner_w)
    visible = lines[max(0, len(lines) - height - state.scroll) : max(0, len(lines) - state.scroll) or None]
    for idx, line in enumerate(visible[:height]):
        color = _line_color(line)
        prefix = _line_prefix(line)
        _addstr(stdscr, y + idx, x + 1, (prefix + line.text)[: inner_w], color)


def _render_details(stdscr, state: TuiState, y: int, x: int, width: int, height: int) -> None:
    _vline(stdscr, y, x, height)
    _addstr(stdscr, y, x + 2, "Details", curses.color_pair(1) | curses.A_BOLD)
    turn = state.active_turn
    trace = trace_label(turn.trace if turn is not None else None)
    rows = [
        ("Session", state.session_id or "-"),
        ("Turn", turn.id if turn else "-"),
        ("Status", turn.status if turn else state.status),
        ("Events", str(len(turn.events) if turn else 0)),
        ("Trace", trace),
    ]
    row_y = y + 2
    for label, value in rows:
        _addstr(stdscr, row_y, x + 2, label, curses.color_pair(2))
        row_y += 1
        for part in textwrap.wrap(str(value), width=max(8, width - 4)) or ["-"]:
            if row_y >= y + height:
                return
            _addstr(stdscr, row_y, x + 2, part, curses.color_pair(3))
            row_y += 1
    if state.active_approval and row_y < y + height - 4:
        row_y += 1
        _addstr(stdscr, row_y, x + 2, "Approval", curses.color_pair(6) | curses.A_BOLD)
        row_y += 1
        approval_lines = _approval_lines(state.active_approval)
        for line in approval_lines:
            if row_y >= y + height:
                return
            _addstr(stdscr, row_y, x + 2, line[: max(8, width - 4)], curses.color_pair(3))
            row_y += 1
    if turn and turn.final_answer and row_y < y + height - 2:
        _addstr(stdscr, row_y + 1, x + 2, "Final", curses.color_pair(2))
        row_y += 2
        for part in textwrap.wrap(turn.final_answer, width=max(8, width - 4)):
            if row_y >= y + height:
                return
            _addstr(stdscr, row_y, x + 2, part, curses.color_pair(3))
            row_y += 1


def _render_input(stdscr, state: TuiState, y: int, x: int, width: int, height: int) -> None:
    del height
    _hline(stdscr, y, x, width)
    if state.active_approval is not None:
        approval = state.active_approval
        title = f"Approval: {approval.get('tool_name') or 'tool'}"
        body = _compact_json(approval.get("tool_input") or {})
        _addstr(stdscr, y + 1, x + 1, title[: max(0, width - 2)], curses.color_pair(6) | curses.A_BOLD)
        _addstr(stdscr, y + 2, x + 1, body[: max(0, width - 2)], curses.color_pair(3))
        return
    prompt = "Task"
    _addstr(stdscr, y + 1, x + 1, prompt, curses.color_pair(1) | curses.A_BOLD)
    input_x = x + len(prompt) + 3
    input_w = max(1, width - input_x - 1)
    value = state.input_buffer[-input_w:]
    _addstr(stdscr, y + 1, input_x, value, curses.color_pair(3))
    try:
        stdscr.move(y + 1, min(input_x + len(value), width - 2))
    except curses.error:
        pass


def _render_footer(stdscr, state: TuiState, y: int, width: int) -> None:
    controls = "Enter send | /help | /sessions | /resume <id> | Ctrl-N new | Ctrl-L clear | PageUp/PageDown scroll | Ctrl-C/Esc/Ctrl-D quit"
    if state.active_approval is not None:
        controls = "approval required: a/y allow | d/n/Esc deny | Ctrl-C deny + interrupt"
    elif state.session_picker_open:
        controls = "session picker: Up/Down or j/k move | Enter resume | Esc close | PageUp/PageDown jump"
    if state.is_running:
        controls = "running... " + controls
    _addstr(stdscr, y, 0, controls[: width - 1], curses.color_pair(2))


def _line_prefix(line: TimelineLine) -> str:
    return {
        "user": "YOU  ",
        "assistant": "AI   ",
        "tool": "TOOL ",
        "warning": "WARN ",
        "error": "ERR  ",
        "step": "STEP ",
        "patch": "DIFF ",
        "trace": "RUN  ",
        "status": "INFO ",
    }.get(line.kind, "EVT  ")


def _line_color(line: TimelineLine) -> int:
    return {
        "assistant": curses.color_pair(3),
        "tool": curses.color_pair(5),
        "warning": curses.color_pair(6) | curses.A_BOLD,
        "error": curses.color_pair(4) | curses.A_BOLD,
        "patch": curses.color_pair(5) | curses.A_BOLD,
        "status": curses.color_pair(1) | (curses.A_BOLD if line.important else 0),
        "user": curses.color_pair(1) | curses.A_BOLD,
    }.get(line.kind, curses.color_pair(2))


def _init_colors() -> None:
    if not curses.has_colors():
        return
    curses.start_color()
    curses.init_pair(1, curses.COLOR_CYAN, -1)
    curses.init_pair(2, curses.COLOR_BLUE, -1)
    curses.init_pair(3, curses.COLOR_WHITE, -1)
    curses.init_pair(4, curses.COLOR_RED, -1)
    curses.init_pair(5, curses.COLOR_GREEN, -1)
    curses.init_pair(6, curses.COLOR_YELLOW, -1)


def _hline(stdscr, y: int, x: int, width: int) -> None:
    _addstr(stdscr, y, x, "-" * max(0, width - x - 1), curses.color_pair(2))


def _vline(stdscr, y: int, x: int, height: int) -> None:
    for row in range(y, y + height):
        _addstr(stdscr, row, x, "|", curses.color_pair(2))


def _addstr(stdscr, y: int, x: int, value: str, attr: int = 0) -> None:
    try:
        stdscr.addstr(y, x, value, attr)
    except curses.error:
        pass


def _approval_lines(approval: dict[str, Any]) -> list[str]:
    lines = [
        f"tool: {approval.get('tool_name') or 'tool'}",
        f"request: {short_id(str(approval.get('request_id') or ''))}",
    ]
    call_id = approval.get("call_id")
    if call_id:
        lines.append(f"call: {short_id(str(call_id))}")
    lines.append(_compact_json(approval.get("tool_input") or {}))
    return lines


def _compact_json(value: Any) -> str:
    try:
        return json.dumps(value, ensure_ascii=False, sort_keys=True)
    except TypeError:
        return str(value)
