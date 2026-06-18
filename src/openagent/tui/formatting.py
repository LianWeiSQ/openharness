from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

from openagent.app_server.protocol import AppEvent


@dataclass(frozen=True, slots=True)
class TimelineLine:
    kind: str
    text: str
    important: bool = False


def short_id(value: str | None, *, keep: int = 12) -> str:
    if not value:
        return "-"
    if len(value) <= keep:
        return value
    return value[:keep] + "..."


def trace_label(trace: dict[str, Any] | None) -> str:
    if not trace:
        return "-"
    return str(trace.get("trace_id") or trace.get("run_id") or trace.get("trace_path") or "-")


def format_event(event: AppEvent) -> list[TimelineLine]:
    method = event.method
    params = event.params
    raw = params.get("event") if isinstance(params.get("event"), dict) else {}
    event_type = str(raw.get("type") or params.get("event_type") or "")

    if method == "turn/started":
        return [TimelineLine("status", f"turn started: {short_id(str(params.get('turn_id') or ''))}", True)]
    if method == "turn/interrupt_requested":
        return [TimelineLine("warning", "interrupt requested", True)]
    if method == "turn/approval_requested":
        approval = params.get("approval") if isinstance(params.get("approval"), dict) else {}
        tool_name = str(approval.get("tool_name") or "tool")
        tool_input = _compact_json(approval.get("tool_input") or {})
        preview = approval.get("preview") if isinstance(approval.get("preview"), dict) else {}
        suffix = _approval_preview_summary(preview)
        return [TimelineLine("warning", f"approval required: {tool_name} {tool_input}{suffix}", True)]
    if method == "turn/approval_resolved":
        approval = params.get("approval") if isinstance(params.get("approval"), dict) else {}
        tool_name = str(approval.get("tool_name") or "tool")
        action = str(approval.get("action") or "-")
        scope = str(approval.get("scope") or "").strip()
        reason = str(approval.get("reason") or "").strip()
        note = str(approval.get("note") or "").strip()
        suffix_parts = [item for item in (scope, reason, note) if item]
        suffix = f" ({'; '.join(suffix_parts)})" if suffix_parts else ""
        kind = "warning" if action == "deny" else "status"
        return [TimelineLine(kind, f"approval {action}: {tool_name}{suffix}", True)]
    if method in {"turn/completed", "turn/failed", "turn/interrupted"}:
        default_status = "interrupted" if method.endswith("interrupted") else ("failed" if method.endswith("failed") else "completed")
        status = str(params.get("status") or default_status)
        lines = [TimelineLine("status", f"turn {status}", important=True)]
        final_answer = str(params.get("final_answer") or "").strip()
        if final_answer:
            lines.append(TimelineLine("assistant", final_answer))
        trace = trace_label(params.get("trace") if isinstance(params.get("trace"), dict) else None)
        if trace != "-":
            lines.append(TimelineLine("trace", f"trace: {trace}"))
        return lines
    if method == "item/patch/reverted":
        patch_hash = short_id(str(params.get("patch_hash") or params.get("patch_ref") or ""))
        reverted = params.get("reverted") if isinstance(params.get("reverted"), list) else []
        skipped = params.get("skipped") if isinstance(params.get("skipped"), list) else []
        lines = [f"patch reverted: {len(reverted)} item(s) hash={patch_hash}"]
        lines.extend(f"- {item}" for item in reverted[:20])
        if skipped:
            lines.append("not reverted:")
            lines.extend(f"- {item}" for item in skipped[:20])
        return [TimelineLine("patch", "\n".join(lines), True)]
    if method == "item/patch/revert_failed":
        patch_hash = short_id(str(params.get("patch_hash") or params.get("patch_ref") or ""))
        error = str(params.get("error") or "").strip()
        skipped = params.get("skipped") if isinstance(params.get("skipped"), list) else []
        lines = [f"patch revert failed: hash={patch_hash}"]
        if error:
            lines.append(error)
        lines.extend(f"- {item}" for item in skipped[:20])
        return [TimelineLine("error", "\n".join(lines), True)]

    if event_type == "text-delta":
        return [TimelineLine("assistant", str(raw.get("text") or ""))]
    if event_type == "tool-call":
        name = str(raw.get("name") or "tool")
        tool_input = _compact_json(raw.get("input") or {})
        return [TimelineLine("tool", f"tool call: {name} {tool_input}", True)]
    if event_type == "tool-result":
        call_id = short_id(str(raw.get("call_id") or ""))
        error = raw.get("error")
        output = str(raw.get("output") or "")
        prefix = f"tool result: {call_id}"
        if error:
            return [TimelineLine("error", f"{prefix} error: {error}", True)]
        return [TimelineLine("tool", f"{prefix}\n{_trim(output)}")]
    if event_type == "runtime-warning":
        code = str(raw.get("code") or "warning")
        message = str(raw.get("message") or "")
        return [TimelineLine("warning", f"warning: {code}\n{message}", True)]
    if event_type == "step-start":
        return [TimelineLine("step", f"step started: snapshot {short_id(str(raw.get('snapshot_id') or ''))}")]
    if event_type == "step-finish":
        tokens = raw.get("tokens") if isinstance(raw.get("tokens"), dict) else {}
        finish = str(raw.get("finish_reason") or "-")
        cost = raw.get("cost")
        return [TimelineLine("step", f"step finished: {finish} tokens={_compact_json(tokens)} cost={cost}")]
    if event_type == "patch":
        files = raw.get("files") if isinstance(raw.get("files"), list) else []
        lines = [f"patch detected: {len(files)} files hash={short_id(str(raw.get('hash') or ''))}"]
        for index, item in enumerate(files[:12], start=1):
            if not isinstance(item, dict):
                continue
            status = str(item.get("status") or "modified")
            path = str(item.get("path") or "-")
            diff = str(item.get("diff") or "")
            added, removed = _diff_stats(diff)
            lines.append(f"{index}. {status} {path} (+{added}/-{removed})")
            if diff.strip():
                lines.extend(f"    {line}" for line in _trim_diff_lines(diff, max_lines=40))
        if len(files) > 12:
            lines.append(f"... {len(files) - 12} more")
        return [TimelineLine("patch", "\n".join(lines), True)]
    if event_type == "error" or method == "turn/error":
        return [TimelineLine("error", f"error: {raw.get('error') or params.get('error')}", True)]
    if event_type in {"text-start", "text-end"}:
        return []
    return [TimelineLine("event", f"{method}: {_compact_json(params)}")]


def wrap_lines(lines: list[TimelineLine], *, width: int) -> list[TimelineLine]:
    if width <= 4:
        return lines
    wrapped: list[TimelineLine] = []
    for line in lines:
        for raw_part in line.text.splitlines() or [""]:
            part = raw_part
            while len(part) > width:
                wrapped.append(TimelineLine(line.kind, part[:width], line.important))
                part = part[width:]
            wrapped.append(TimelineLine(line.kind, part, line.important))
    return wrapped


def _compact_json(value: Any) -> str:
    try:
        return json.dumps(value, ensure_ascii=False, sort_keys=True)
    except TypeError:
        return str(value)


def _trim(value: str, *, limit: int = 1200) -> str:
    if len(value) <= limit:
        return value
    return value[:limit].rstrip() + "\n... truncated ..."


def _diff_stats(value: str) -> tuple[int, int]:
    added = 0
    removed = 0
    for line in value.splitlines():
        if line.startswith("+++") or line.startswith("---"):
            continue
        if line.startswith("+"):
            added += 1
        elif line.startswith("-"):
            removed += 1
    return added, removed


def _trim_diff_lines(value: str, *, max_lines: int) -> list[str]:
    lines = value.splitlines()
    if len(lines) <= max_lines:
        return lines
    return [*lines[:max_lines], f"... diff truncated ({len(lines) - max_lines} more lines) ..."]


def _approval_preview_summary(preview: dict[str, Any]) -> str:
    if not preview:
        return ""
    kind = str(preview.get("kind") or "tool")
    path = str(preview.get("path") or "")
    status = str(preview.get("status") or "")
    bits = [kind]
    if path:
        bits.append(path)
    if status:
        bits.append(status)
    diff = str(preview.get("diff") or "")
    if diff:
        added, removed = _diff_stats(diff)
        bits.append(f"+{added}/-{removed}")
    warnings = preview.get("warnings") if isinstance(preview.get("warnings"), list) else []
    if warnings:
        bits.append("warning")
    return "\npreview: " + " ".join(bits)
