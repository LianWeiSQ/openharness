from __future__ import annotations

import json
from pathlib import Path
from typing import Any


def load_trace_events(path: str | Path) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        item = json.loads(line)
        if isinstance(item, dict):
            events.append(item)
    return events


def summarize_trace(path: str | Path) -> dict[str, Any]:
    events = load_trace_events(path)
    errors = [event for event in events if event.get("status") == "error"]
    model_events = [event for event in events if _event_name(event) == "model.call.finished"]
    tool_events = [event for event in events if _event_name(event) == "tool.call.finished"]
    context_events = [event for event in events if _event_name(event).startswith("context.")]
    input_tokens = 0
    output_tokens = 0
    cost = 0.0
    usage_events = [
        event
        for event in events
        if _event_name(event) == "model.call.finished"
        and isinstance(event.get("attributes"), dict)
        and (
            event["attributes"].get("input_tokens") is not None
            or event["attributes"].get("output_tokens") is not None
            or event["attributes"].get("cost") is not None
        )
    ]
    if not usage_events:
        usage_events = [event for event in events if _event_name(event) == "model.usage"]
    for event in usage_events:
        attrs = event.get("attributes")
        if not isinstance(attrs, dict):
            continue
        input_tokens += int(attrs.get("input_tokens") or 0)
        output_tokens += int(attrs.get("output_tokens") or 0)
        cost += float(attrs.get("cost") or 0.0)
    return {
        "event_count": len(events),
        "error_count": len(errors),
        "model_call_count": len(model_events),
        "tool_call_count": len(tool_events),
        "context_event_count": len(context_events),
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cost": cost,
        "first_error": errors[0] if errors else None,
    }


def render_trace_summary(path: str | Path) -> str:
    summary = summarize_trace(path)
    lines = [
        "# OpenAgent Trace Summary",
        "",
        f"- Events: {summary['event_count']}",
        f"- Errors: {summary['error_count']}",
        f"- Model calls: {summary['model_call_count']}",
        f"- Tool calls: {summary['tool_call_count']}",
        f"- Context events: {summary['context_event_count']}",
        f"- Input tokens: {summary['input_tokens']}",
        f"- Output tokens: {summary['output_tokens']}",
        f"- Cost: {summary['cost']:.6f}",
    ]
    if summary["first_error"]:
        error = summary["first_error"]
        attrs = error.get("attributes") if isinstance(error, dict) else {}
        lines.extend(
            [
                "",
                "## First Error",
                "",
                f"- Event: {_event_name(error)}",
                f"- Kind: {attrs.get('error_kind') if isinstance(attrs, dict) else ''}",
                f"- Message: {attrs.get('message') if isinstance(attrs, dict) else ''}",
            ]
        )
    return "\n".join(lines) + "\n"


def _event_name(event: dict[str, Any]) -> str:
    return str(event.get("event") or event.get("name") or "")


__all__ = ["load_trace_events", "render_trace_summary", "summarize_trace"]
