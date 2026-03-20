from __future__ import annotations

from dataclasses import replace
from pathlib import Path
from time import time
from typing import Any

from .context_budget import estimate_message_tokens
from .types import ChatMessage, Model, ToolResult

CONTEXT_COMPACTION_METADATA_KEY = "context_compaction"
SYNTHETIC_COMPACTION_HEADER = "[Compacted context summary]"


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

    summary = raw.get("summary")
    compacted_until = raw.get("compacted_until")
    if not isinstance(summary, str) or not summary.strip():
        return None
    if not isinstance(compacted_until, int):
        return None
    if compacted_until <= 0 or compacted_until > message_count:
        return None

    updated_at = raw.get("updated_at")
    return {
        "summary": summary.strip(),
        "compacted_until": compacted_until,
        "updated_at": int(updated_at) if isinstance(updated_at, int) else 0,
    }


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
        content=f"{SYNTHETIC_COMPACTION_HEADER}\n{compaction['summary']}",
        metadata={
            "synthetic_context_compaction": True,
            "compacted_until": compacted_until,
        },
    )
    return [summary_message, *messages[compacted_until:]]


def build_trimmed_messages_for_model(
    messages: list[ChatMessage],
    metadata: dict[str, Any],
    *,
    keep_recent_user_turns: int,
) -> list[ChatMessage]:
    compaction = get_context_compaction(metadata, message_count=len(messages))
    boundary = recent_user_turn_start(messages, keep_recent_user_turns)
    if compaction is None:
        return list(messages[boundary:])
    compacted_until = int(compaction["compacted_until"])
    summary_message = ChatMessage(
        role="assistant",
        content=f"{SYNTHETIC_COMPACTION_HEADER}\n{compaction['summary']}",
        metadata={
            "synthetic_context_compaction": True,
            "compacted_until": compacted_until,
        },
    )
    start = max(boundary, compacted_until)
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

    count_value = metadata.get("count")
    if not isinstance(count_value, int):
        count_value = metadata.get("returned_count") if isinstance(metadata.get("returned_count"), int) else 0
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
