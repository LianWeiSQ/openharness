from __future__ import annotations

import argparse
import curses
import os
import textwrap
import time
from pathlib import Path

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
    if key in {3}:  # Ctrl-C
        if state.is_running:
            state.request_interrupt()
            return False
        return True
    if key in {4}:  # Ctrl-D
        return True
    if key == 27 and not state.is_running and not state.input_buffer:  # Esc
        return True
    if key in {14}:  # Ctrl-N
        state.new_session()
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
    _addstr(stdscr, y, 1, "Sessions", curses.color_pair(1) | curses.A_BOLD)
    try:
        sessions = state.runtime.list_sessions()[: max(0, height - 3)]
    except Exception as error:  # noqa: BLE001
        sessions = [{"id": "error", "status": str(error), "message_count": 0}]
    for idx, session in enumerate(sessions, start=1):
        sid = str(session.get("id") or "-")
        marker = "*" if sid == state.session_id else " "
        line = f"{marker} {short_id(sid, keep=16)}"
        _addstr(stdscr, y + idx, 1, line[: width - 2], curses.color_pair(3 if marker == "*" else 2))
        meta = f"  {session.get('message_count') or 0} msg"
        _addstr(stdscr, y + idx + 1, 1, meta[: width - 2], curses.color_pair(2))
    _vline(stdscr, y, width - 1, height)


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
    controls = "Enter send | Ctrl-N new | Ctrl-L clear | PageUp/PageDown scroll | Ctrl-C/Esc/Ctrl-D quit"
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
