from __future__ import annotations

import argparse
import curses
import json
import textwrap
import time
from pathlib import Path
from typing import Any

from openagent.app_server.runtime import OpenAgentAppRuntime

from .formatting import TimelineLine, short_id, trace_label, wrap_lines
from .state import TuiState


def main(
    argv: list[str] | None = None,
    *,
    runtime: object | None = None,
    initial_session_id: str | None = None,
    continue_last: bool = False,
) -> None:
    parser = argparse.ArgumentParser(description="Run the OpenAgent terminal UI.")
    parser.add_argument("--workspace", default=None)
    parser.add_argument("--session-root", default=None)
    parser.add_argument("--session", "-s", default=None)
    parser.add_argument("--continue", "-c", dest="continue_last", action="store_true")
    args = parser.parse_args(argv)

    active_runtime = runtime or OpenAgentAppRuntime(workspace=args.workspace, session_store_root=args.session_root)
    state = TuiState(runtime=active_runtime)
    _apply_initial_session(
        state,
        initial_session_id=initial_session_id or args.session,
        continue_last=continue_last or bool(args.continue_last),
    )
    try:
        curses.wrapper(lambda stdscr: _run(stdscr, state))
    except KeyboardInterrupt:
        return


def _apply_initial_session(state: TuiState, *, initial_session_id: str | None, continue_last: bool) -> None:
    if initial_session_id:
        state.resume_session(initial_session_id)
        return
    if not continue_last:
        return
    try:
        sessions = list(state.runtime.list_sessions())
    except Exception as error:  # noqa: BLE001 - startup failures should stay visible inside the TUI.
        state.timeline.append(TimelineLine("error", f"failed to continue latest session: {error}", important=True))
        state.status = "continue failed"
        return
    if sessions:
        session_id = str(sessions[0].get("id") or "")
        if session_id:
            state.resume_session(session_id)


def _run(stdscr, state: TuiState) -> None:
    curses.curs_set(1)
    curses.use_default_colors()
    _init_colors()
    stdscr.timeout(120)
    state.ensure_session()

    while True:
        state.poll_events()
        _drain_control_requests(state)
        _render(stdscr, state)
        key = stdscr.getch()
        if key == -1:
            continue
        if _handle_key(key, state):
            break


def _handle_key(key: int, state: TuiState) -> bool:
    if state.active_approval is not None:
        if state.approval_note_mode:
            if key in {10, 13}:
                state.respond_approval("deny", note=state.approval_note.strip() or None)
                return False
            if key == 27:
                state.cancel_approval_note()
                return False
            if key in {curses.KEY_BACKSPACE, 127, 8}:
                state.approval_note = state.approval_note[:-1]
                return False
            if 32 <= key <= 126:
                state.approval_note += chr(key)
                return False
            return False
        if key in {ord("a"), ord("y"), ord("Y")}:
            state.respond_approval("allow", scope="once")
            return False
        if key == ord("A"):
            state.respond_approval("allow", scope="always")
            return False
        if key in {ord("d"), ord("D"), ord("n"), ord("N"), 27}:
            state.respond_approval("deny")
            return False
        if key in {ord("r"), ord("R")}:
            state.start_approval_note()
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
    if key == 27 and state.model_picker_open:  # Esc
        state.close_model_picker()
        return False
    if key == 27 and state.agent_picker_open:  # Esc
        state.close_agent_picker()
        return False
    if key == 27 and state.variant_picker_open:  # Esc
        state.close_variant_picker()
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
    if state.model_picker_open:
        if key in {10, 13}:
            state.select_model_from_picker()
            return False
        if key in {curses.KEY_UP, ord("k")}:
            state.move_model_picker(-1)
            return False
        if key in {curses.KEY_DOWN, ord("j")}:
            state.move_model_picker(1)
            return False
        if key == curses.KEY_PPAGE:
            state.move_model_picker(-5)
            return False
        if key == curses.KEY_NPAGE:
            state.move_model_picker(5)
            return False
    if state.agent_picker_open:
        if key in {10, 13}:
            state.select_agent_from_picker()
            return False
        if key in {curses.KEY_UP, ord("k")}:
            state.move_agent_picker(-1)
            return False
        if key in {curses.KEY_DOWN, ord("j")}:
            state.move_agent_picker(1)
            return False
        if key == curses.KEY_PPAGE:
            state.move_agent_picker(-5)
            return False
        if key == curses.KEY_NPAGE:
            state.move_agent_picker(5)
            return False
    if state.variant_picker_open:
        if key in {10, 13}:
            state.select_variant_from_picker()
            return False
        if key in {curses.KEY_UP, ord("k")}:
            state.move_variant_picker(-1)
            return False
        if key in {curses.KEY_DOWN, ord("j")}:
            state.move_variant_picker(1)
            return False
        if key == curses.KEY_PPAGE:
            state.move_variant_picker(-5)
            return False
        if key == curses.KEY_NPAGE:
            state.move_variant_picker(5)
            return False
    if state.file_picker_open:
        if key in {10, 13, 9}:  # Enter or Tab
            state.select_file_mention()
            return False
        if key in {27}:  # Esc
            state.close_file_picker()
            return False
        if key == curses.KEY_UP:
            state.move_file_picker(-1)
            return False
        if key == curses.KEY_DOWN:
            state.move_file_picker(1)
            return False
        if key == curses.KEY_PPAGE:
            state.move_file_picker(-5)
            return False
        if key == curses.KEY_NPAGE:
            state.move_file_picker(5)
            return False
    if key in {14}:  # Ctrl-N
        state.new_session()
        return False
    if key in {19}:  # Ctrl-S
        state.stash_current_draft()
        return False
    if key in {16}:  # Ctrl-P
        state.pop_draft_stash()
        return False
    if key in {18}:  # Ctrl-R
        state.open_session_picker(announce=True)
        return False
    if key in {12}:  # Ctrl-L
        state.clear()
        return False
    if key in {10, 13}:
        state.submit()
        state.close_file_picker(update_status=False)
        return False
    if key in {curses.KEY_BACKSPACE, 127, 8}:
        state.backspace_input()
        return False
    if key == curses.KEY_UP:
        state.prompt_history_previous()
        return False
    if key == curses.KEY_DOWN:
        state.prompt_history_next()
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
        state.append_input_char(chr(key))
    return False


def _drain_control_requests(state: TuiState) -> None:
    drain = getattr(state.runtime, "drain_control_requests", None)
    if not callable(drain):
        return
    try:
        requests = drain()
    except Exception as error:  # noqa: BLE001 - attached control failures should stay visible.
        state.timeline.append(TimelineLine("error", f"TUI control failed: {error}", important=True))
        state.status = "control failed"
        return
    post_response = getattr(state.runtime, "post_control_response", None)
    for request in requests:
        try:
            result = state.apply_control_request(request)
        except Exception as error:  # noqa: BLE001 - keep the TUI alive on bad remote control messages.
            state.timeline.append(TimelineLine("error", f"TUI control failed: {error}", important=True))
            state.status = "control failed"
            if callable(post_response):
                post_response({"path": str(request.get("path") or ""), "error": str(error)}, ok=False)
            continue
        if callable(post_response):
            post_response({"path": str(request.get("path") or "")}, ok=True, result=result)


def _render(stdscr, state: TuiState) -> None:
    stdscr.erase()
    height, width = stdscr.getmaxyx()
    if height < 8 or width < 40:
        _addstr(stdscr, 0, 0, "OpenAgent TUI needs at least 40x8 terminal size.", curses.color_pair(4))
        stdscr.refresh()
        return

    header_h = 3
    input_h = 8 if state.file_picker_open or state.model_picker_open or state.agent_picker_open or state.variant_picker_open else 4
    if state.active_approval is not None:
        input_h = min(12, max(6, height - header_h - 2))
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
    selector = f"{state.agent_label}/{state.model_label}/{state.variant_label}"
    status = state.status
    _addstr(stdscr, 0, 0, title, curses.color_pair(1) | curses.A_BOLD)
    _addstr(stdscr, 0, len(title) + 2, f"session {session}", curses.color_pair(2))
    right = f"{status} | {selector}"
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
        ("Model", state.model_label),
        ("Agent", state.agent_label),
        ("Variant", state.variant_label),
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
    _hline(stdscr, y, x, width)
    if state.active_approval is not None:
        approval = state.active_approval
        title = f"Approval: {approval.get('tool_name') or 'tool'}"
        _addstr(stdscr, y + 1, x + 1, title[: max(0, width - 2)], curses.color_pair(6) | curses.A_BOLD)
        body_lines = _approval_lines(approval)
        available = max(0, height - 3)
        for idx, line in enumerate(body_lines[:available]):
            _addstr(stdscr, y + 2 + idx, x + 1, line[: max(0, width - 2)], curses.color_pair(3))
        if state.approval_note_mode and height >= 3:
            note = f"Deny note: {state.approval_note}"
            _addstr(stdscr, y + height - 1, x + 1, note[: max(0, width - 2)], curses.color_pair(6) | curses.A_BOLD)
        return
    prompt = "Task"
    _addstr(stdscr, y + 1, x + 1, prompt, curses.color_pair(1) | curses.A_BOLD)
    input_x = x + len(prompt) + 3
    input_w = max(1, width - input_x - 1)
    value = state.input_buffer[-input_w:]
    _addstr(stdscr, y + 1, input_x, value, curses.color_pair(3))
    if state.file_picker_open:
        _render_file_picker(stdscr, state, y + 2, x + 1, width - 2, max(0, height - 3))
    elif state.model_picker_open:
        _render_model_picker(stdscr, state, y + 2, x + 1, width - 2, max(0, height - 3))
    elif state.agent_picker_open:
        _render_agent_picker(stdscr, state, y + 2, x + 1, width - 2, max(0, height - 3))
    elif state.variant_picker_open:
        _render_variant_picker(stdscr, state, y + 2, x + 1, width - 2, max(0, height - 3))
    try:
        stdscr.move(y + 1, min(input_x + len(value), width - 2))
    except curses.error:
        pass


def _render_footer(stdscr, state: TuiState, y: int, width: int) -> None:
    controls = "Enter send | Up/Down history | Ctrl-S stash | Ctrl-P pop | /help | Ctrl-N new | Ctrl-L clear | PageUp/PageDown scroll"
    if state.active_approval is not None:
        controls = "approval note: type reason | Enter deny | Esc cancel" if state.approval_note_mode else "approval: a/y allow once | A always | d/n/Esc deny | r deny note | Ctrl-C deny + interrupt"
    elif state.file_picker_open:
        controls = "file picker: Up/Down move | Enter/Tab insert | Esc close"
    elif state.session_picker_open:
        controls = "session picker: Up/Down or j/k move | Enter resume | Esc close | PageUp/PageDown jump"
    elif state.model_picker_open:
        controls = "model picker: Up/Down or j/k move | Enter select | Esc close | PageUp/PageDown jump"
    elif state.agent_picker_open:
        controls = "agent picker: Up/Down or j/k move | Enter select | Esc close | PageUp/PageDown jump"
    elif state.variant_picker_open:
        controls = "variant picker: Up/Down or j/k move | Enter select | Esc close | PageUp/PageDown jump"
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


def _render_file_picker(stdscr, state: TuiState, y: int, x: int, width: int, height: int) -> None:
    if height <= 0:
        return
    title = f"Files @{state.file_picker_query}"
    _addstr(stdscr, y, x, title[:width], curses.color_pair(6) | curses.A_BOLD)
    for idx, path in enumerate(state.file_picker_matches[: max(0, height - 1)]):
        marker = ">" if idx == state.file_picker_index else " "
        attr = curses.color_pair(1) | curses.A_BOLD if idx == state.file_picker_index else curses.color_pair(3)
        _addstr(stdscr, y + idx + 1, x, f"{marker} @{path}"[:width], attr)


def _render_model_picker(stdscr, state: TuiState, y: int, x: int, width: int, height: int) -> None:
    if height <= 0:
        return
    _addstr(stdscr, y, x, "Models", curses.color_pair(6) | curses.A_BOLD)
    for idx, model in enumerate(state.model_picker_models[: max(0, height - 1)]):
        marker = ">" if idx == state.model_picker_index else ("*" if str(model.get("id") or "") == (state.selected_model_id or "") else " ")
        provider = str(model.get("provider_id") or "-")
        model_id = str(model.get("id") or "-")
        name = str(model.get("name") or model_id)
        label = f"{marker} {provider}/{model_id}  {name}"
        attr = curses.color_pair(1) | curses.A_BOLD if idx == state.model_picker_index else curses.color_pair(3)
        _addstr(stdscr, y + idx + 1, x, label[:width], attr)


def _render_agent_picker(stdscr, state: TuiState, y: int, x: int, width: int, height: int) -> None:
    if height <= 0:
        return
    _addstr(stdscr, y, x, "Agents", curses.color_pair(6) | curses.A_BOLD)
    for idx, agent in enumerate(state.agent_picker_agents[: max(0, height - 1)]):
        marker = ">" if idx == state.agent_picker_index else ("*" if agent == state.agent_label else " ")
        attr = curses.color_pair(1) | curses.A_BOLD if idx == state.agent_picker_index else curses.color_pair(3)
        _addstr(stdscr, y + idx + 1, x, f"{marker} {agent}"[:width], attr)


def _render_variant_picker(stdscr, state: TuiState, y: int, x: int, width: int, height: int) -> None:
    if height <= 0:
        return
    _addstr(stdscr, y, x, "Variants", curses.color_pair(6) | curses.A_BOLD)
    for idx, variant in enumerate(state.variant_picker_variants[: max(0, height - 1)]):
        marker = ">" if idx == state.variant_picker_index else ("*" if variant == state.variant_label else " ")
        attr = curses.color_pair(1) | curses.A_BOLD if idx == state.variant_picker_index else curses.color_pair(3)
        _addstr(stdscr, y + idx + 1, x, f"{marker} {variant}"[:width], attr)


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
    preview = approval.get("preview") if isinstance(approval.get("preview"), dict) else {}
    if preview:
        lines.extend(_approval_preview_lines(preview))
    lines.append("input: " + _compact_json(approval.get("tool_input") or {}))
    return lines


def _approval_preview_lines(preview: dict[str, Any]) -> list[str]:
    lines = [f"preview: {preview.get('kind') or 'tool'}"]
    path = preview.get("path")
    if path:
        status = preview.get("status")
        suffix = f" ({status})" if status else ""
        lines.append(f"path: {path}{suffix}")
    command = preview.get("command")
    if command:
        lines.append(f"command: {command}")
    warnings = preview.get("warnings") if isinstance(preview.get("warnings"), list) else []
    for warning in warnings[:3]:
        lines.append(f"warning: {warning}")
    diff = str(preview.get("diff") or "").strip()
    if diff:
        lines.append("diff:")
        diff_lines = diff.splitlines()
        lines.extend(diff_lines[:40])
        if len(diff_lines) > 40:
            lines.append(f"... diff truncated ({len(diff_lines) - 40} more lines) ...")
    summary = str(preview.get("summary") or "").strip()
    if summary:
        lines.append(f"summary: {summary}")
    return lines


def _compact_json(value: Any) -> str:
    try:
        return json.dumps(value, ensure_ascii=False, sort_keys=True)
    except TypeError:
        return str(value)
