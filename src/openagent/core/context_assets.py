from __future__ import annotations

import hashlib
import time
from pathlib import Path
from typing import Any, Sequence

CONTEXT_ASSET_SNAPSHOTS_METADATA_KEY = "context_asset_snapshots"
LAST_CONTEXT_ASSETS_METADATA_KEY = "last_context_assets_snapshot"
SESSION_MEMORY_METADATA_KEY = "session_memory"


def build_context_assets_snapshot(
    *,
    step_index: int,
    instruction_context: Any | None,
    file_context_state: Any,
) -> dict[str, Any]:
    instruction_asset = instruction_context_to_asset(instruction_context)
    file_asset = file_context_state_to_asset(file_context_state)
    return {
        "schema_version": "openagent.context_assets_snapshot.v1",
        "step_index": step_index,
        "instructions": instruction_asset,
        "files": file_asset,
    }


def instruction_context_to_asset(context: Any | None) -> dict[str, Any]:
    if context is None:
        return {
            "schema_version": "openagent.instruction_snapshot.v1",
            "item_count": 0,
            "total_bytes": 0,
            "truncated": False,
            "issues": ["unavailable"],
            "items": [],
        }
    return {
        "schema_version": "openagent.instruction_snapshot.v1",
        "item_count": len(context.items),
        "total_bytes": context.total_bytes,
        "truncated": context.truncated,
        "issues": list(context.issues),
        "items": [
            {
                "path": str(item.path),
                "display_path": item.display_path,
                "source": item.source,
                "scope": item.scope,
                "bytes_read": item.bytes_read,
                "truncated": item.truncated,
                "content_hash": _sha256_text(item.content),
                "content_chars": len(item.content),
            }
            for item in context.items
        ],
    }


def file_context_state_to_asset(state: Any) -> dict[str, Any]:
    changes = state.changed_records()
    return {
        "schema_version": "openagent.file_context_snapshot.v1",
        "record_count": len(state.records),
        "changed_count": len(changes),
        "records": [
            record.to_dict()
            for record in sorted(state.records.values(), key=lambda item: (item.path, item.absolute_path))
        ],
        "changes": [
            {
                "path": change.record.path,
                "absolute_path": change.record.absolute_path,
                "exists": change.exists,
                "changed": change.changed,
                "reason": change.reason,
                "content_hash": change.record.content_hash,
            }
            for change in changes
        ],
    }


def render_session_memory(
    *,
    session_id: str,
    workspace: str | Path,
    messages: Sequence[Any],
    todos: Sequence[Any],
    metadata: dict[str, Any],
    step_index: int | None = None,
) -> str:
    lines = [
        "# OpenAgent Session Memory",
        "",
        f"- session_id: `{session_id}`",
        f"- workspace: `{workspace}`",
        f"- updated_at_ms: `{int(time.time() * 1000)}`",
    ]
    if step_index is not None:
        lines.append(f"- last_step_index: `{step_index}`")
    lines.extend(["", "## Work State"])
    compaction = metadata.get("context_compaction") if isinstance(metadata.get("context_compaction"), dict) else None
    if compaction is not None:
        summary = compaction.get("summary")
        if not isinstance(summary, str) or not summary.strip():
            summary = _compact_state_summary(compaction.get("state"))
        lines.append(str(summary or "").strip() or "- (empty)")
    else:
        lines.append("- No structured compaction has been created yet.")

    lines.extend(["", "## Todos"])
    if todos:
        for todo in todos:
            status = getattr(todo, "status", "pending")
            priority = getattr(todo, "priority", "medium")
            content = getattr(todo, "content", "")
            lines.append(f"- [{status}] ({priority}) {content}")
    else:
        lines.append("- (none)")

    lines.extend(["", "## Recent Messages"])
    for message in list(messages)[-8:]:
        role = getattr(message, "role", "message")
        name = getattr(message, "name", None)
        label = role if name is None else f"{role}:{name}"
        text = _single_line(getattr(message, "content", ""), max_chars=240)
        lines.append(f"- `{label}` {text}")

    file_asset = _file_context_metadata_to_asset(metadata)
    lines.extend(["", "## File Context"])
    if file_asset["records"]:
        for record in file_asset["records"][-10:]:
            lines.append(f"- `{record['path']}` hash={record['content_hash']} source={record['source_tool']}")
    else:
        lines.append("- (none)")

    last_pack = metadata.get("last_context_pack_snapshot")
    if isinstance(last_pack, dict) and last_pack.get("snapshot_path"):
        lines.extend(["", "## Latest Context Pack"])
        lines.append(f"- snapshot: `{last_pack['snapshot_path']}`")
        lines.append(f"- estimated_input_tokens: `{last_pack.get('estimated_input_tokens')}`")
        lines.append(f"- included_count: `{last_pack.get('included_count')}`")

    return "\n".join(lines).rstrip() + "\n"


def _sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def _single_line(value: str, *, max_chars: int) -> str:
    text = " ".join(str(value or "").split())
    if len(text) <= max_chars:
        return text or "(empty)"
    return text[: max_chars - 3] + "..."


def _compact_state_summary(value: Any) -> str:
    if not isinstance(value, dict):
        return ""
    parts: list[str] = []
    task = value.get("task")
    if isinstance(task, str) and task.strip():
        parts.append(f"Task: {task.strip()}")
    for key in ("progress", "next_steps", "blockers", "risks"):
        raw = value.get(key)
        if isinstance(raw, list) and raw:
            parts.append(f"{key}: " + "; ".join(str(item) for item in raw[:5]))
    return "\n".join(parts)


def _file_context_metadata_to_asset(metadata: dict[str, Any]) -> dict[str, Any]:
    raw = metadata.get("file_context_state")
    records_raw = raw.get("records") if isinstance(raw, dict) and isinstance(raw.get("records"), dict) else {}
    records = [dict(value) for value in records_raw.values() if isinstance(value, dict)]
    return {
        "schema_version": "openagent.file_context_snapshot.v1",
        "record_count": len(records),
        "changed_count": 0,
        "records": sorted(records, key=lambda item: (str(item.get("path") or ""), str(item.get("absolute_path") or ""))),
        "changes": [],
    }


__all__ = [
    "CONTEXT_ASSET_SNAPSHOTS_METADATA_KEY",
    "LAST_CONTEXT_ASSETS_METADATA_KEY",
    "SESSION_MEMORY_METADATA_KEY",
    "build_context_assets_snapshot",
    "file_context_state_to_asset",
    "instruction_context_to_asset",
    "render_session_memory",
]
