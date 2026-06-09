from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from .recorder import (
    DEFAULT_TRACE_ROOT,
    check_trace_run,
    find_run_dir,
    list_runs,
    load_trace_events,
    load_trace_summary,
    render_trace_summary,
)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="openagent-trace", description="Inspect OpenAgent run traces.")
    parser.add_argument("--root", default=DEFAULT_TRACE_ROOT, help="Trace root directory. Defaults to .openagent/runs.")
    sub = parser.add_subparsers(dest="command", required=True)

    list_parser = sub.add_parser("list", help="List trace runs.")
    list_parser.add_argument("--json", action="store_true", help="Render raw JSON.")

    show_parser = sub.add_parser("show", help="Show a run summary.")
    show_parser.add_argument("run_id")
    show_parser.add_argument("--json", action="store_true", help="Render raw JSON.")

    summary_parser = sub.add_parser("summary", help="Show a run summary.")
    summary_parser.add_argument("run_id")
    summary_parser.add_argument("--json", action="store_true", help="Render raw JSON.")

    events_parser = sub.add_parser("events", help="Show run events.")
    events_parser.add_argument("run_id")
    events_parser.add_argument("--json", action="store_true", help="Render JSON lines.")
    events_parser.add_argument("--limit", type=int, default=50, help="Maximum events to render.")

    check_parser = sub.add_parser("check", help="Validate a run trace contains the P0 event closure.")
    check_parser.add_argument("run_id")
    check_parser.add_argument("--json", action="store_true", help="Render raw JSON.")

    args = parser.parse_args(argv)
    if args.command == "list":
        return _cmd_list(root=args.root, render_json=bool(args.json))
    if args.command in {"show", "summary"}:
        return _cmd_show(run_id=args.run_id, root=args.root, render_json=bool(args.json))
    if args.command == "events":
        return _cmd_events(run_id=args.run_id, root=args.root, render_json=bool(args.json), limit=int(args.limit))
    if args.command == "check":
        return _cmd_check(run_id=args.run_id, root=args.root, render_json=bool(args.json))
    parser.error("Unknown command")
    return 2


def _cmd_list(*, root: str, render_json: bool) -> int:
    runs = list_runs(root=root)
    if render_json:
        print(json.dumps(runs, ensure_ascii=False, indent=2, sort_keys=True))
        return 0
    if not runs:
        print("No trace runs found.")
        return 0
    for summary in runs:
        print(
            f"{summary.get('run_id')}  status={summary.get('status')} "
            f"events={summary.get('event_count', 0)} tools={summary.get('tool_call_count', 0)} "
            f"models={summary.get('model_call_count', 0)} cost={float(summary.get('total_cost') or 0.0):.6f}"
        )
    return 0


def _cmd_show(*, run_id: str, root: str, render_json: bool) -> int:
    run_dir = find_run_dir(run_id, root=root)
    summary = load_trace_summary(run_dir / "summary.json")
    if render_json:
        print(json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True))
    else:
        print(render_trace_summary(summary), end="")
        paths = summary.get("paths") if isinstance(summary.get("paths"), dict) else {}
        trace_path = paths.get("trace") or str(run_dir / "trace.jsonl")
        print(f"Trace: {trace_path}")
    return 0


def _cmd_events(*, run_id: str, root: str, render_json: bool, limit: int) -> int:
    run_dir = find_run_dir(run_id, root=root)
    events = load_trace_events(run_dir / "trace.jsonl")
    selected = events[-max(1, limit) :]
    if render_json:
        for event in selected:
            print(json.dumps(event, ensure_ascii=False, sort_keys=True))
        return 0
    for event in selected:
        attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
        label = _event_label(event, attrs)
        status = " error" if event.get("status") == "error" else ""
        duration = f" {event.get('duration_ms')}ms" if event.get("duration_ms") is not None else ""
        print(f"{event.get('seq')}. {event.get('event')}{status}{duration}{label}")
    return 0


def _cmd_check(*, run_id: str, root: str, render_json: bool) -> int:
    run_dir = find_run_dir(run_id, root=root)
    result = check_trace_run(run_dir)
    if render_json:
        print(json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True))
    elif result["ok"]:
        print(f"Trace OK: {result.get('run_id')} events={result.get('event_count')}")
    else:
        print(f"Trace FAILED: {result.get('run_id')}")
        for error in result["errors"]:
            print(f"- {error}")
    return 0 if result["ok"] else 1


def _event_label(event: dict[str, Any], attrs: dict[str, Any]) -> str:
    name = str(event.get("event") or "")
    if name.startswith("model.call"):
        bits = []
        if attrs.get("finish_reason"):
            bits.append(f"finish={attrs.get('finish_reason')}")
        if attrs.get("input_tokens") is not None or attrs.get("output_tokens") is not None:
            bits.append(f"tokens={attrs.get('input_tokens', 0)}/{attrs.get('output_tokens', 0)}")
        return " " + " ".join(bits) if bits else ""
    if name.startswith("tool.call"):
        tool_name = attrs.get("tool_name")
        source = attrs.get("tool_source") or attrs.get("backend") or attrs.get("tool_group")
        suffix = f" {source}.{tool_name}" if source and tool_name else f" {tool_name}" if tool_name else ""
        if attrs.get("error_kind"):
            suffix += f" error_kind={attrs.get('error_kind')}"
        return suffix
    if name.startswith("step."):
        return f" step={attrs.get('step_index')}" if attrs.get("step_index") is not None else ""
    return ""


if __name__ == "__main__":
    raise SystemExit(main())
