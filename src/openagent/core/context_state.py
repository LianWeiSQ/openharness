from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

STRUCTURED_WORK_STATE_SCHEMA_VERSION = 1
STRUCTURED_WORK_STATE_FORMAT = "structured_work_state"
STRUCTURED_WORK_STATE_HEADER = "[Structured work state]"

WORK_STATE_LIST_FIELDS = (
    "progress",
    "decisions",
    "tool_findings",
    "todos",
    "open_questions",
    "blockers",
    "next_steps",
    "risks",
)
WORK_STATE_FILE_STATUSES = frozenset({"read", "modified", "created", "deleted", "mentioned", "unknown"})

MAX_TASK_CHARS = 1200
MAX_LIST_ITEMS = 24
MAX_LIST_ITEM_CHARS = 1000
MAX_FILES = 32
MAX_FILE_PATH_CHARS = 500
MAX_FILE_NOTE_CHARS = 1000
MAX_LEGACY_TEXT_CHARS = 4000


@dataclass(frozen=True, slots=True)
class ParsedWorkState:
    state: dict[str, Any]
    summary: str
    source: str
    parse_error: str | None = None


def parse_work_state_output(raw_text: str) -> ParsedWorkState:
    """Parse provider output into a bounded structured work state.

    Providers are not required to support JSON mode. This parser therefore accepts
    fenced JSON, embedded JSON, and legacy free-form text.
    """

    text = str(raw_text or "").strip()
    if not text:
        raise ValueError("Structured work state compaction produced no text.")

    parse_error: str | None = None
    candidate = _extract_json_object(text)
    if candidate is not None:
        state_source = _state_source_from_json(candidate)
        state = normalize_work_state(state_source)
        if _state_has_content(state):
            return ParsedWorkState(
                state=state,
                summary=render_work_state(state),
                source="model_json",
                parse_error=None,
            )

        fallback = _coerce_text(candidate.get("summary"), max_chars=MAX_LEGACY_TEXT_CHARS) or text
        parse_error = "JSON object did not contain actionable work-state fields."
        return _legacy_fallback(fallback, parse_error=parse_error)

    parse_error = "No JSON object found in compaction output."
    return _legacy_fallback(text, parse_error=parse_error)


def build_compaction_record(
    *,
    raw_text: str,
    compacted_until: int,
    updated_at: int,
) -> dict[str, Any]:
    parsed = parse_work_state_output(raw_text)
    record: dict[str, Any] = {
        "schema_version": STRUCTURED_WORK_STATE_SCHEMA_VERSION,
        "format": STRUCTURED_WORK_STATE_FORMAT,
        "state": parsed.state,
        "summary": parsed.summary,
        "compacted_until": compacted_until,
        "updated_at": updated_at,
        "source": parsed.source,
    }
    if parsed.parse_error:
        record["parse_error"] = parsed.parse_error
    return record


def normalize_work_state(raw: Any) -> dict[str, Any]:
    data = raw if isinstance(raw, dict) else {}
    task = _coerce_text(
        _first_present(data, "task", "goal", "current_task", "objective", "summary"),
        max_chars=MAX_TASK_CHARS,
    )
    state: dict[str, Any] = {
        "task": task,
        "progress": _coerce_text_list(_first_present(data, "progress", "completed", "completed_work")),
        "decisions": _coerce_text_list(_first_present(data, "decisions", "constraints")),
        "files": _coerce_files(_first_present(data, "files", "important_files", "artifacts")),
        "tool_findings": _coerce_text_list(_first_present(data, "tool_findings", "evidence", "findings")),
        "todos": _coerce_text_list(_first_present(data, "todos", "todo", "todo_list")),
        "open_questions": _coerce_text_list(_first_present(data, "open_questions", "questions")),
        "blockers": _coerce_text_list(_first_present(data, "blockers", "blocked_by")),
        "next_steps": _coerce_text_list(_first_present(data, "next_steps", "next", "recommended_next_steps")),
        "risks": _coerce_text_list(_first_present(data, "risks", "verification_gaps", "gaps")),
    }
    return state


def render_work_state(state: dict[str, Any]) -> str:
    normalized = normalize_work_state(state)
    lines: list[str] = [STRUCTURED_WORK_STATE_HEADER, "Task:", normalized["task"] or "(unspecified)"]

    _append_section(lines, "Progress", normalized["progress"])
    _append_section(lines, "Decisions", normalized["decisions"])
    _append_files_section(lines, normalized["files"])
    _append_section(lines, "Tool findings", normalized["tool_findings"])
    _append_section(lines, "Todos", normalized["todos"])
    _append_section(lines, "Open questions", normalized["open_questions"])
    _append_section(lines, "Blockers", normalized["blockers"])
    _append_section(lines, "Next steps", normalized["next_steps"])
    _append_section(lines, "Risks", normalized["risks"])
    return "\n".join(lines).strip()


def render_work_state_brief(state: dict[str, Any]) -> str:
    normalized = normalize_work_state(state)
    lines = [
        STRUCTURED_WORK_STATE_HEADER,
        f"Task: {normalized['task'] or '(unspecified)'}",
    ]
    if normalized["next_steps"]:
        lines.append("Next: " + "; ".join(normalized["next_steps"][:3]))
    if normalized["blockers"]:
        lines.append("Blockers: " + "; ".join(normalized["blockers"][:3]))
    if normalized["open_questions"]:
        lines.append("Questions: " + "; ".join(normalized["open_questions"][:3]))
    return "\n".join(lines).strip()


def _legacy_fallback(text: str, *, parse_error: str) -> ParsedWorkState:
    summary = _coerce_text(text, max_chars=MAX_LEGACY_TEXT_CHARS)
    state = normalize_work_state({"task": summary, "progress": [], "next_steps": []})
    return ParsedWorkState(
        state=state,
        summary=render_work_state(state),
        source="legacy_text_fallback",
        parse_error=parse_error,
    )


def _extract_json_object(text: str) -> dict[str, Any] | None:
    stripped = text.strip()
    direct = _try_json_object(stripped)
    if direct is not None:
        return direct

    fenced = _extract_fenced_json(stripped)
    if fenced is not None:
        return fenced

    decoder = json.JSONDecoder()
    for index, char in enumerate(stripped):
        if char != "{":
            continue
        try:
            value, _end = decoder.raw_decode(stripped, index)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            return value
    return None


def _try_json_object(text: str) -> dict[str, Any] | None:
    try:
        value = json.loads(text)
    except json.JSONDecodeError:
        return None
    return value if isinstance(value, dict) else None


def _extract_fenced_json(text: str) -> dict[str, Any] | None:
    fence = "```"
    start = text.find(fence)
    while start >= 0:
        content_start = text.find("\n", start + len(fence))
        if content_start < 0:
            return None
        end = text.find(fence, content_start + 1)
        if end < 0:
            return None
        block = text[content_start + 1 : end].strip()
        parsed = _try_json_object(block)
        if parsed is not None:
            return parsed
        start = text.find(fence, end + len(fence))
    return None


def _state_source_from_json(data: dict[str, Any]) -> dict[str, Any]:
    for key in ("state", "work_state", "structured_work_state", "context"):
        value = data.get(key)
        if isinstance(value, dict):
            return value
    return data


def _state_has_content(state: dict[str, Any]) -> bool:
    if str(state.get("task") or "").strip():
        return True
    if state.get("files"):
        return True
    return any(bool(state.get(field)) for field in WORK_STATE_LIST_FIELDS)


def _first_present(data: dict[str, Any], *keys: str) -> Any:
    for key in keys:
        if key in data:
            return data[key]
    return None


def _coerce_text(value: Any, *, max_chars: int) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        text = value
    elif isinstance(value, (int, float, bool)):
        text = str(value)
    else:
        text = json.dumps(value, ensure_ascii=False, sort_keys=True)
    text = " ".join(text.split())
    if len(text) <= max_chars:
        return text
    return text[: max_chars - 3].rstrip() + "..."


def _coerce_text_list(value: Any) -> list[str]:
    items: list[Any]
    if value is None:
        items = []
    elif isinstance(value, list):
        items = value
    elif isinstance(value, str):
        items = _split_text_list(value)
    else:
        items = [value]

    result: list[str] = []
    seen: set[str] = set()
    for item in items:
        text = _coerce_text(item, max_chars=MAX_LIST_ITEM_CHARS)
        text = _strip_bullet_prefix(text)
        if not text or text in seen:
            continue
        seen.add(text)
        result.append(text)
        if len(result) >= MAX_LIST_ITEMS:
            break
    return result


def _split_text_list(text: str) -> list[str]:
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if len(lines) <= 1:
        return [text]
    return lines


def _strip_bullet_prefix(text: str) -> str:
    stripped = text.strip()
    for prefix in ("- ", "* ", "+ "):
        if stripped.startswith(prefix):
            return stripped[len(prefix) :].strip()
    if len(stripped) > 3 and stripped[0].isdigit() and stripped[1] in {".", ")"}:
        return stripped[2:].strip()
    return stripped


def _coerce_files(value: Any) -> list[dict[str, str]]:
    items: list[Any]
    if value is None:
        items = []
    elif isinstance(value, list):
        items = value
    else:
        items = [value]

    result: list[dict[str, str]] = []
    seen: set[str] = set()
    for item in items:
        file_item = _coerce_file(item)
        path = file_item["path"]
        if not path or path in seen:
            continue
        seen.add(path)
        result.append(file_item)
        if len(result) >= MAX_FILES:
            break
    return result


def _coerce_file(value: Any) -> dict[str, str]:
    if isinstance(value, dict):
        path = _coerce_text(_first_present(value, "path", "file", "name"), max_chars=MAX_FILE_PATH_CHARS)
        status = _coerce_text(value.get("status"), max_chars=64).lower() or "unknown"
        if status not in WORK_STATE_FILE_STATUSES:
            status = "unknown"
        note = _coerce_text(_first_present(value, "note", "reason", "summary", "description"), max_chars=MAX_FILE_NOTE_CHARS)
        return {"path": path, "status": status, "note": note}

    text = _coerce_text(value, max_chars=MAX_FILE_PATH_CHARS + MAX_FILE_NOTE_CHARS)
    path, note = _split_file_text(text)
    return {
        "path": _coerce_text(path, max_chars=MAX_FILE_PATH_CHARS),
        "status": "mentioned",
        "note": _coerce_text(note, max_chars=MAX_FILE_NOTE_CHARS),
    }


def _split_file_text(text: str) -> tuple[str, str]:
    for separator in (" - ", ": "):
        if separator in text:
            left, right = text.split(separator, 1)
            return left.strip(), right.strip()
    return text.strip(), ""


def _append_section(lines: list[str], title: str, items: list[str]) -> None:
    if not items:
        return
    lines.append("")
    lines.append(f"{title}:")
    lines.extend(f"- {item}" for item in items)


def _append_files_section(lines: list[str], files: list[dict[str, str]]) -> None:
    if not files:
        return
    lines.append("")
    lines.append("Files:")
    for item in files:
        suffix = f" - {item['note']}" if item.get("note") else ""
        lines.append(f"- {item['path']} ({item['status']}){suffix}")


__all__ = [
    "ParsedWorkState",
    "STRUCTURED_WORK_STATE_FORMAT",
    "STRUCTURED_WORK_STATE_HEADER",
    "STRUCTURED_WORK_STATE_SCHEMA_VERSION",
    "build_compaction_record",
    "normalize_work_state",
    "parse_work_state_output",
    "render_work_state_brief",
    "render_work_state",
]
