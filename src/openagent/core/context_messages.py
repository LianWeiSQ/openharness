from __future__ import annotations

from dataclasses import replace
from pathlib import Path
from time import time
from typing import Any

from .context_budget import estimate_message_tokens
from .context_state import (
    STRUCTURED_WORK_STATE_FORMAT,
    STRUCTURED_WORK_STATE_HEADER,
    normalize_work_state,
    render_work_state,
    render_work_state_brief,
)
from .types import ChatMessage, Model, ToolResult

CONTEXT_COMPACTION_METADATA_KEY = "context_compaction"
SYNTHETIC_COMPACTION_HEADER = "[Compacted context summary]"
OVERFLOW_TOOL_PREVIEW_BYTES = 1024
OVERFLOW_TOOL_PREVIEW_LINES = 8
OVERFLOW_TOOL_LINE_MAX_CHARS = 160


def recent_user_turn_start(messages: list[ChatMessage], keep_recent_user_turns: int) -> int:
    if keep_recent_user_turns <= 0:
        return len(messages)

    seen = 0
    for index in range(len(messages) - 1, -1, -1):
        if messages[index].role != "user":
            continue
        seen += 1
        if seen == keep_recent_user_turns:
            return index
    return 0


def get_context_compaction(metadata: dict[str, Any], *, message_count: int) -> dict[str, Any] | None:
    raw = metadata.get(CONTEXT_COMPACTION_METADATA_KEY)
    if not isinstance(raw, dict):
        return None

    compacted_until = raw.get("compacted_until")
    if not isinstance(compacted_until, int):
        return None
    if compacted_until <= 0 or compacted_until > message_count:
        return None

    summary = _render_compaction_summary(raw)
    if not summary:
        return None

    updated_at = raw.get("updated_at")
    result = {
        "summary": summary.strip(),
        "compacted_until": compacted_until,
        "updated_at": int(updated_at) if isinstance(updated_at, int) else 0,
    }
    for key in ("schema_version", "format", "state", "source", "parse_error"):
        if key in raw:
            result[key] = raw[key]
    return result


def count_new_messages_since_compaction(messages: list[ChatMessage], metadata: dict[str, Any]) -> int:
    compaction = get_context_compaction(metadata, message_count=len(messages))
    if compaction is None:
        return 0
    return max(len(messages) - int(compaction["compacted_until"]), 0)


def build_messages_for_model(messages: list[ChatMessage], metadata: dict[str, Any]) -> list[ChatMessage]:
    compaction = get_context_compaction(metadata, message_count=len(messages))
    if compaction is None:
        return list(messages)

    compacted_until = int(compaction["compacted_until"])
    summary_message = ChatMessage(
        role="assistant",
        content=_format_compaction_message(compaction),
        metadata={
            "synthetic_context_compaction": True,
            "compacted_until": compacted_until,
            "format": compaction.get("format"),
            "schema_version": compaction.get("schema_version"),
        },
    )
    return [summary_message, *messages[compacted_until:]]


def build_trimmed_messages_for_model(
    messages: list[ChatMessage],
    metadata: dict[str, Any],
    *,
    keep_recent_user_turns: int,
    compact_tool_messages: bool = False,
) -> list[ChatMessage]:
    compaction = get_context_compaction(metadata, message_count=len(messages))
    boundary = recent_user_turn_start(messages, keep_recent_user_turns)
    if compaction is None:
        trimmed_messages = list(messages[boundary:])
        return _overflow_compacted_messages(trimmed_messages) if compact_tool_messages else trimmed_messages
    compacted_until = int(compaction["compacted_until"])
    summary_message = ChatMessage(
        role="assistant",
        content=_format_compaction_message(compaction),
        metadata={
            "synthetic_context_compaction": True,
            "compacted_until": compacted_until,
            "format": compaction.get("format"),
            "schema_version": compaction.get("schema_version"),
        },
    )
    start = max(boundary, compacted_until)
    trimmed_messages = list(messages[start:])
    if compact_tool_messages:
        trimmed_messages = _overflow_compacted_messages(trimmed_messages)
    return [summary_message, *trimmed_messages]


def build_brief_messages_for_model(messages: list[ChatMessage], metadata: dict[str, Any]) -> list[ChatMessage]:
    compaction = get_context_compaction(metadata, message_count=len(messages))
    if compaction is None:
        return list(messages)

    state = compaction.get("state")
    if not isinstance(state, dict):
        return build_messages_for_model(messages, metadata)

    compacted_until = int(compaction["compacted_until"])
    summary_message = ChatMessage(
        role="assistant",
        content=render_work_state_brief(normalize_work_state(state)),
        metadata={
            "synthetic_context_compaction": True,
            "compacted_until": compacted_until,
            "format": compaction.get("format"),
            "schema_version": compaction.get("schema_version"),
            "brief": True,
        },
    )
    return [summary_message, *messages[compacted_until:]]


def build_brief_trimmed_messages_for_model(
    messages: list[ChatMessage],
    metadata: dict[str, Any],
    *,
    keep_recent_user_turns: int,
) -> list[ChatMessage]:
    compaction = get_context_compaction(metadata, message_count=len(messages))
    if compaction is None:
        return list(messages[recent_user_turn_start(messages, keep_recent_user_turns) :])

    state = compaction.get("state")
    if not isinstance(state, dict):
        return build_trimmed_messages_for_model(
            messages,
            metadata,
            keep_recent_user_turns=keep_recent_user_turns,
            compact_tool_messages=True,
        )

    compacted_until = int(compaction["compacted_until"])
    summary_message = ChatMessage(
        role="assistant",
        content=render_work_state_brief(normalize_work_state(state)),
        metadata={
            "synthetic_context_compaction": True,
            "compacted_until": compacted_until,
            "format": compaction.get("format"),
            "schema_version": compaction.get("schema_version"),
            "brief": True,
            "trimmed": True,
        },
    )
    start = max(recent_user_turn_start(messages, keep_recent_user_turns), compacted_until)
    return [summary_message, *messages[start:]]


def project_tool_result_to_message(
    *,
    result: ToolResult,
    tool_name: str,
    session_root: Path,
    preview_bytes: int,
    preview_lines: int,
    line_max_chars: int,
) -> tuple[ToolResult, ChatMessage]:
    metadata = dict(result.metadata or {})
    metadata.setdefault("tool", tool_name)
    metadata.setdefault("title", metadata.get("title") or "")

    display_output = result.output or ""
    output_path = metadata.get("output_path")
    if not isinstance(output_path, str) and len(display_output.encode("utf-8")) > preview_bytes:
        output_path = str(_write_tool_output(session_root, result.call_id, display_output))
        metadata["output_path"] = output_path

    preview_source = metadata.get("preview")
    if not isinstance(preview_source, str) or not preview_source.strip():
        preview_source = f"ERROR: {result.error}" if result.error else display_output
    preview = _build_preview(
        preview_source,
        max_bytes=preview_bytes,
        max_lines=preview_lines,
        line_max_chars=line_max_chars,
    )
    metadata["context_preview"] = preview

    count_value = metadata.get("count")
    if not isinstance(count_value, int):
        count_value = metadata.get("returned_count") if isinstance(metadata.get("returned_count"), int) else 0

    original_lines = metadata.get("original_lines")
    if not isinstance(original_lines, int):
        original_lines = display_output.count("\n") + (1 if display_output else 0)
    original_bytes = metadata.get("original_bytes")
    if not isinstance(original_bytes, int):
        original_bytes = len(display_output.encode("utf-8")) if display_output else 0

    lines = [
        f"[Tool result] {tool_name}",
        f"title={metadata.get('title', '')}",
        f"status={'error' if result.error else 'ok'}",
        f"truncated={bool(metadata.get('truncated'))} original_lines={original_lines} original_bytes={original_bytes} count={count_value}",
        "preview:",
        preview,
    ]
    if isinstance(output_path, str) and output_path:
        lines.append(f"full_output={output_path}")

    updated_result = replace(result, metadata=metadata)
    tool_message = ChatMessage(
        role="tool",
        name=tool_name,
        tool_call_id=result.call_id,
        content="\n".join(lines),
        metadata=metadata,
    )
    return updated_result, tool_message


def prune_old_tool_messages(
    messages: list[ChatMessage],
    *,
    bytes_per_token: int,
    keep_recent_user_turns: int,
    protect_input_tokens: int,
    min_input_tokens: int,
    model: Model | None = None,
    options: dict[str, Any] | None = None,
    counting: str = "heuristic",
) -> tuple[list[ChatMessage], int]:
    if not messages:
        return list(messages), 0

    boundary = recent_user_turn_start(messages, keep_recent_user_turns)
    if boundary <= 0:
        return list(messages), 0

    total = 0
    reclaimed = 0
    replacements: dict[int, ChatMessage] = {}

    for index in range(len(messages) - 1, -1, -1):
        if index >= boundary:
            continue
        message = messages[index]
        if message.role != "tool":
            continue
        if bool((message.metadata or {}).get("compacted")):
            continue

        estimate = estimate_message_tokens(
            message,
            bytes_per_token=bytes_per_token,
            model=model,
            options=options,
            counting=counting,
        )
        total += estimate
        if total <= protect_input_tokens:
            continue

        reclaimed += estimate
        replacements[index] = _compact_tool_message(message)

    if reclaimed < min_input_tokens:
        return list(messages), 0

    new_messages = list(messages)
    for index, replacement in replacements.items():
        new_messages[index] = replacement
    return new_messages, reclaimed


def _compact_tool_message(message: ChatMessage) -> ChatMessage:
    metadata = dict(message.metadata or {})
    metadata["compacted"] = True
    metadata["compacted_at"] = int(time() * 1000)

    count_value = _tool_count_value(metadata)
    original_bytes = metadata.get("original_bytes")
    if not isinstance(original_bytes, int):
        original_bytes = len(message.content.encode("utf-8")) if message.content else 0

    lines = [
        "[Old tool result content cleared]",
        f"tool={message.name or metadata.get('tool', '')}",
        f"title={metadata.get('title', '')}",
        f"count={count_value}",
        f"truncated={bool(metadata.get('truncated'))}",
        f"full_output={metadata.get('output_path') or 'unavailable'}",
        f"original_bytes={original_bytes}",
    ]
    return ChatMessage(
        role="tool",
        name=message.name,
        tool_call_id=message.tool_call_id,
        content="\n".join(lines),
        metadata=metadata,
    )


def _render_compaction_summary(raw: dict[str, Any]) -> str:
    if raw.get("format") == STRUCTURED_WORK_STATE_FORMAT and isinstance(raw.get("state"), dict):
        return render_work_state(normalize_work_state(raw["state"]))
    summary = raw.get("summary")
    if isinstance(summary, str) and summary.strip():
        return summary.strip()
    if isinstance(raw.get("state"), dict):
        return render_work_state(normalize_work_state(raw["state"]))
    return ""


def _format_compaction_message(compaction: dict[str, Any]) -> str:
    summary = str(compaction.get("summary") or "").strip()
    if summary.startswith(STRUCTURED_WORK_STATE_HEADER):
        return summary
    return f"{SYNTHETIC_COMPACTION_HEADER}\n{summary}"


def _overflow_compacted_messages(messages: list[ChatMessage]) -> list[ChatMessage]:
    compacted: list[ChatMessage] = []
    for message in messages:
        if message.role != "tool":
            compacted.append(message)
            continue
        compacted.append(_compact_tool_message_for_overflow(message))
    return compacted


def _compact_tool_message_for_overflow(message: ChatMessage) -> ChatMessage:
    metadata = dict(message.metadata or {})
    metadata["overflow_context_compacted"] = True

    preview_source = metadata.get("context_preview")
    if not isinstance(preview_source, str) or not preview_source.strip():
        preview_source = metadata.get("preview")
    if not isinstance(preview_source, str) or not preview_source.strip():
        preview_source = message.content
    preview = _build_preview(
        str(preview_source),
        max_bytes=OVERFLOW_TOOL_PREVIEW_BYTES,
        max_lines=OVERFLOW_TOOL_PREVIEW_LINES,
        line_max_chars=OVERFLOW_TOOL_LINE_MAX_CHARS,
    )

    lines = [
        "[Overflow tool context summary]",
        f"tool={message.name or metadata.get('tool', '')}",
        f"title={metadata.get('title', '')}",
        f"status={_tool_status(message, metadata)}",
        f"count={_tool_count_value(metadata)}",
        "preview:",
        preview,
        f"full_output={metadata.get('output_path') or 'unavailable'}",
    ]
    return ChatMessage(
        role="tool",
        name=message.name,
        tool_call_id=message.tool_call_id,
        content="\n".join(lines),
        metadata=metadata,
    )


def _tool_count_value(metadata: dict[str, Any]) -> int:
    count_value = metadata.get("count")
    if isinstance(count_value, int):
        return count_value
    returned_count = metadata.get("returned_count")
    if isinstance(returned_count, int):
        return returned_count
    return 0


def _tool_status(message: ChatMessage, metadata: dict[str, Any]) -> str:
    if isinstance(metadata.get("error_kind"), str) and metadata.get("error_kind"):
        return "error"
    if "status=error" in message.content:
        return "error"
    return "ok"


def _build_preview(text: str, *, max_bytes: int, max_lines: int, line_max_chars: int) -> str:
    if not text:
        return "(empty)"

    collected: list[str] = []
    bytes_used = 0
    for line in text.splitlines()[:max_lines]:
        shortened = line if len(line) <= line_max_chars else line[:line_max_chars] + "..."
        encoded_size = len(shortened.encode("utf-8")) + (1 if collected else 0)
        if bytes_used + encoded_size > max_bytes:
            break
        collected.append(shortened)
        bytes_used += encoded_size

    if collected:
        return "\n".join(collected)

    encoded = text.encode("utf-8")[:max_bytes]
    return encoded.decode("utf-8", errors="ignore") or "(empty)"


def _write_tool_output(session_root: Path, call_id: str, content: str) -> Path:
    out_dir = session_root / ".openagent" / "tool_output"
    out_dir.mkdir(parents=True, exist_ok=True)
    output_path = out_dir / f"{call_id}.txt"
    output_path.write_text(content, encoding="utf-8")
    return output_path
