from __future__ import annotations

import json
import time
from abc import ABC, abstractmethod
from dataclasses import asdict, is_dataclass
from pathlib import Path
from typing import Any

from ..id import new_id
from ..types import ChatMessage, SessionStatus
from .session import Session
from .todo import todos_from_payload, todos_to_dicts

SESSION_STORE_METADATA_KEY = "session_store"
DEFAULT_SESSION_STORE_ROOT = ".openagent/sessions"


class SessionStore(ABC):
    """Persistent source of truth for resumable Agent sessions."""

    @abstractmethod
    def start_run(
        self,
        session: Session,
        *,
        run_id: str,
        trace_id: str,
        agent_name: str,
        model_id: str | None,
        provider_id: str | None,
        permission: str,
        max_steps: int,
        started_at_ms: int | None = None,
    ) -> dict[str, Any]:
        raise NotImplementedError

    @abstractmethod
    def append_message(self, session: Session, message: ChatMessage, *, run_id: str, index: int) -> None:
        raise NotImplementedError

    @abstractmethod
    def record_event(
        self,
        *,
        session_id: str,
        run_id: str,
        event: str,
        kind: str = "event",
        status: str = "ok",
        attributes: dict[str, Any] | None = None,
        duration_ms: int | None = None,
    ) -> None:
        raise NotImplementedError

    @abstractmethod
    def append_part(
        self,
        *,
        session_id: str,
        run_id: str,
        part_type: str,
        attributes: dict[str, Any] | None = None,
        step_index: int | None = None,
        status: str = "ok",
    ) -> dict[str, Any]:
        raise NotImplementedError

    @abstractmethod
    def record_context_pack(
        self,
        *,
        session_id: str,
        run_id: str,
        snapshot: dict[str, Any],
    ) -> dict[str, Any]:
        raise NotImplementedError

    @abstractmethod
    def record_context_assets(
        self,
        *,
        session_id: str,
        run_id: str,
        snapshot: dict[str, Any],
    ) -> dict[str, Any]:
        raise NotImplementedError

    @abstractmethod
    def record_session_memory(
        self,
        *,
        session_id: str,
        run_id: str,
        content: str,
        step_index: int | None = None,
    ) -> dict[str, Any]:
        raise NotImplementedError

    @abstractmethod
    def finish_run(
        self,
        session: Session,
        *,
        run_id: str,
        status: str,
        steps: int,
        finish_reason: str | None = None,
        error: str | None = None,
    ) -> None:
        raise NotImplementedError

    @abstractmethod
    def save_state(self, session: Session, *, run_id: str | None = None) -> None:
        raise NotImplementedError

    @abstractmethod
    def load_session(self, session_id: str) -> Session:
        raise NotImplementedError


class FileSessionStore(SessionStore):
    """Append-only JSONL session ledger plus a latest-state snapshot."""

    def __init__(self, root: str | Path) -> None:
        self.root = Path(root)

    @classmethod
    def from_options(cls, options: dict[str, Any] | None, *, base_dir: str | Path | None = None) -> "FileSessionStore | None":
        raw_options = options or {}
        raw = raw_options.get("session_store", {})
        if raw is False:
            return None
        if raw is None:
            raw = {}
        if not isinstance(raw, dict):
            raw = {}
        if not _bool_option(raw.get("enabled", True)):
            return None
        root = Path(str(raw.get("root_dir") or DEFAULT_SESSION_STORE_ROOT))
        if not root.is_absolute():
            root = Path(base_dir or Path.cwd()) / root
        return cls(root)

    def start_run(
        self,
        session: Session,
        *,
        run_id: str,
        trace_id: str,
        agent_name: str,
        model_id: str | None,
        provider_id: str | None,
        permission: str,
        max_steps: int,
        started_at_ms: int | None = None,
    ) -> dict[str, Any]:
        started = started_at_ms or _now_ms()
        self._session_dir(session.id).mkdir(parents=True, exist_ok=True)
        self._run_dir(session.id, run_id).mkdir(parents=True, exist_ok=True)
        session_record = {
            "schema_version": "openagent.session.v1",
            "session_id": session.id,
            "workspace": str(session.directory),
            "status": _status_value(session.status),
            "created_at_ms": started,
            "updated_at_ms": started,
            "active_run_id": run_id,
        }
        run_record = {
            "schema_version": "openagent.run.v1",
            "session_id": session.id,
            "run_id": run_id,
            "trace_id": trace_id,
            "agent_name": agent_name,
            "model_id": model_id,
            "provider_id": provider_id,
            "permission": permission,
            "max_steps": max_steps,
            "status": "running",
            "started_at_ms": started,
            "ended_at_ms": None,
        }
        self._write_json(self._session_json_path(session.id), session_record)
        self._write_json(self._run_json_path(session.id, run_id), run_record)
        metadata = self._metadata(session.id, run_id)
        session.metadata[SESSION_STORE_METADATA_KEY] = metadata
        self._append_index({"event": "run.started", "session_id": session.id, "run_id": run_id, "timestamp_ms": started})
        self.record_event(
            session_id=session.id,
            run_id=run_id,
            event="run.started",
            kind="run",
            attributes={
                "agent_name": agent_name,
                "model_id": model_id,
                "provider_id": provider_id,
                "permission": permission,
                "max_steps": max_steps,
            },
        )
        self.save_state(session, run_id=run_id)
        return metadata

    def append_message(self, session: Session, message: ChatMessage, *, run_id: str, index: int) -> None:
        message_id = _message_id(message)
        payload = {
            "schema_version": "openagent.message.v1",
            "message_id": message_id,
            "session_id": session.id,
            "run_id": run_id,
            "index": index,
            "role": message.role,
            "content": message.content,
            "name": message.name,
            "tool_call_id": message.tool_call_id,
            "metadata": _jsonable(message.metadata),
            "timestamp_ms": _now_ms(),
        }
        self._append_jsonl(self._transcript_path(session.id), payload)
        self.record_event(
            session_id=session.id,
            run_id=run_id,
            event="message.appended",
            kind="message",
            attributes={
                "message_id": message_id,
                "index": index,
                "role": message.role,
                "content_chars": len(message.content or ""),
                "tool_call_id": message.tool_call_id,
            },
        )

    def record_event(
        self,
        *,
        session_id: str,
        run_id: str,
        event: str,
        kind: str = "event",
        status: str = "ok",
        attributes: dict[str, Any] | None = None,
        duration_ms: int | None = None,
    ) -> None:
        event_path = self._events_path(session_id, run_id)
        payload = {
            "schema_version": "openagent.session_event.v1",
            "seq": self._next_seq(event_path),
            "event": event,
            "timestamp_ms": _now_ms(),
            "session_id": session_id,
            "run_id": run_id,
            "kind": kind,
            "status": "error" if status == "error" else "ok",
            "duration_ms": duration_ms,
            "attributes": _jsonable(attributes or {}),
        }
        self._append_jsonl(event_path, payload)
        self._write_run_summary(session_id=session_id, run_id=run_id)

    def append_part(
        self,
        *,
        session_id: str,
        run_id: str,
        part_type: str,
        attributes: dict[str, Any] | None = None,
        step_index: int | None = None,
        status: str = "ok",
    ) -> dict[str, Any]:
        parts_path = self._parts_path(session_id, run_id)
        payload = {
            "schema_version": "openagent.session_part.v1",
            "part_id": new_id("part"),
            "seq": self._next_seq(parts_path),
            "type": part_type,
            "timestamp_ms": _now_ms(),
            "session_id": session_id,
            "run_id": run_id,
            "step_index": step_index,
            "status": "error" if status == "error" else "ok",
            "attributes": _jsonable(attributes or {}),
        }
        self._append_jsonl(parts_path, payload)
        self._write_run_summary(session_id=session_id, run_id=run_id)
        return {
            "schema_version": payload["schema_version"],
            "session_id": session_id,
            "run_id": run_id,
            "parts_path": str(parts_path),
            "part_id": payload["part_id"],
            "seq": payload["seq"],
            "type": part_type,
            "step_index": step_index,
        }

    def record_context_pack(
        self,
        *,
        session_id: str,
        run_id: str,
        snapshot: dict[str, Any],
    ) -> dict[str, Any]:
        step_index = _optional_int(snapshot.get("step_index"))
        filename = f"context-pack-step-{step_index:04d}.json" if step_index is not None else f"context-pack-{_now_ms()}.json"
        context_dir = self._context_pack_dir(session_id, run_id)
        path = context_dir / filename
        payload = {
            "schema_version": "openagent.context_pack_snapshot.v1",
            "session_id": session_id,
            "run_id": run_id,
            "timestamp_ms": _now_ms(),
            **_jsonable(snapshot),
        }
        self._write_json(path, payload)
        metadata = {
            "schema_version": payload["schema_version"],
            "session_id": session_id,
            "run_id": run_id,
            "step_index": step_index,
            "snapshot_path": str(path),
            "context_dir": str(context_dir),
            "item_count": payload.get("item_count"),
            "included_count": payload.get("included_count"),
            "estimated_input_tokens": payload.get("estimated_input_tokens"),
        }
        self.record_event(
            session_id=session_id,
            run_id=run_id,
            event="context.pack_snapshot.saved",
            kind="context",
            attributes=metadata,
        )
        return metadata

    def record_context_assets(
        self,
        *,
        session_id: str,
        run_id: str,
        snapshot: dict[str, Any],
    ) -> dict[str, Any]:
        step_index = _optional_int(snapshot.get("step_index"))
        filename = f"context-assets-step-{step_index:04d}.json" if step_index is not None else f"context-assets-{_now_ms()}.json"
        context_dir = self._context_asset_dir(session_id, run_id)
        path = context_dir / filename
        payload = {
            "schema_version": "openagent.context_assets_snapshot.v1",
            "session_id": session_id,
            "run_id": run_id,
            "timestamp_ms": _now_ms(),
            **_jsonable(snapshot),
        }
        self._write_json(path, payload)
        instructions = payload.get("instructions") if isinstance(payload.get("instructions"), dict) else {}
        files = payload.get("files") if isinstance(payload.get("files"), dict) else {}
        metadata = {
            "schema_version": payload["schema_version"],
            "session_id": session_id,
            "run_id": run_id,
            "step_index": step_index,
            "asset_path": str(path),
            "asset_dir": str(context_dir),
            "instruction_count": instructions.get("item_count", 0),
            "file_record_count": files.get("record_count", 0),
            "file_changed_count": files.get("changed_count", 0),
        }
        self.record_event(
            session_id=session_id,
            run_id=run_id,
            event="context.assets_snapshot.saved",
            kind="context",
            attributes=metadata,
        )
        return metadata

    def record_session_memory(
        self,
        *,
        session_id: str,
        run_id: str,
        content: str,
        step_index: int | None = None,
    ) -> dict[str, Any]:
        path = self._session_memory_path(session_id)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        metadata = {
            "schema_version": "openagent.session_memory.v1",
            "session_id": session_id,
            "run_id": run_id,
            "step_index": step_index,
            "memory_path": str(path),
            "content_chars": len(content),
            "updated_at_ms": _now_ms(),
        }
        self.record_event(
            session_id=session_id,
            run_id=run_id,
            event="session.memory.updated",
            kind="memory",
            attributes=metadata,
        )
        return metadata

    def finish_run(
        self,
        session: Session,
        *,
        run_id: str,
        status: str,
        steps: int,
        finish_reason: str | None = None,
        error: str | None = None,
    ) -> None:
        ended = _now_ms()
        self.record_event(
            session_id=session.id,
            run_id=run_id,
            event="run.finished" if status == "completed" else "run.failed",
            kind="run",
            status="ok" if status == "completed" else "error",
            attributes={
                "status": status,
                "steps": steps,
                "finish_reason": finish_reason,
                "error": error,
            },
        )
        run_path = self._run_json_path(session.id, run_id)
        run_record = self._read_json(run_path) or {}
        run_record.update({"status": status, "ended_at_ms": ended, "steps": steps, "finish_reason": finish_reason, "error": error})
        started_at = _optional_int(run_record.get("started_at_ms")) or ended
        run_record["duration_ms"] = max(0, ended - started_at)
        self._write_json(run_path, run_record)
        session_record = self._read_json(self._session_json_path(session.id)) or {}
        session_record.update({"status": _status_value(session.status), "updated_at_ms": ended, "active_run_id": run_id})
        self._write_json(self._session_json_path(session.id), session_record)
        self.save_state(session, run_id=run_id)

    def save_state(self, session: Session, *, run_id: str | None = None) -> None:
        state = {
            "schema_version": "openagent.session_state.v1",
            "session_id": session.id,
            "run_id": run_id,
            "workspace": str(session.directory),
            "status": _status_value(session.status),
            "updated_at_ms": _now_ms(),
            "messages": [_message_to_dict(message, index=index) for index, message in enumerate(session.messages)],
            "todos": todos_to_dicts(session.todos),
            "metadata": _jsonable(session.metadata),
        }
        self._write_json(self._state_path(session.id), state)

    def load_session(self, session_id: str) -> Session:
        state = self._read_json(self._state_path(session_id))
        if state is None:
            state = self._reconstruct_state_from_transcript(session_id)
        if state is None:
            raise FileNotFoundError(f"Session state not found: {session_id}")
        status_raw = str(state.get("status") or SessionStatus.IDLE.value)
        try:
            status = SessionStatus(status_raw)
        except ValueError:
            status = SessionStatus.IDLE
        messages = [_message_from_dict(item) for item in state.get("messages") or [] if isinstance(item, dict)]
        todos = todos_from_payload([item for item in state.get("todos") or [] if isinstance(item, dict)])
        metadata = dict(state.get("metadata") or {}) if isinstance(state.get("metadata"), dict) else {}
        return Session(
            id=str(state.get("session_id") or session_id),
            directory=Path(str(state.get("workspace") or Path.cwd())),
            status=status,
            messages=messages,
            todos=todos,
            metadata=metadata,
        )

    def _metadata(self, session_id: str, run_id: str) -> dict[str, Any]:
        return {
            "enabled": True,
            "type": "file",
            "root_dir": str(self.root),
            "session_id": session_id,
            "run_id": run_id,
            "session_dir": str(self._session_dir(session_id)),
            "ledger_path": str(self._events_path(session_id, run_id)),
            "transcript_path": str(self._transcript_path(session_id)),
            "state_path": str(self._state_path(session_id)),
            "run_dir": str(self._run_dir(session_id, run_id)),
            "parts_path": str(self._parts_path(session_id, run_id)),
        }

    def _write_run_summary(self, *, session_id: str, run_id: str) -> None:
        events = _read_jsonl(self._events_path(session_id, run_id))
        parts = _read_jsonl(self._parts_path(session_id, run_id))
        summary = {
            "schema_version": "openagent.run_summary.v1",
            "session_id": session_id,
            "run_id": run_id,
            "event_count": len(events),
            "part_count": len(parts),
            "part_type_counts": _count_by_key(parts, "type"),
            "message_count": sum(1 for event in events if event.get("event") == "message.appended"),
            "step_count": sum(1 for event in events if event.get("event") == "step.finished"),
            "tool_call_count": sum(1 for event in events if event.get("event") in {"tool.call.finished", "tool.call.failed"}),
            "runtime_warning_count": sum(1 for event in events if event.get("event") == "runtime.warning"),
            "patch_count": sum(1 for event in events if event.get("event") == "patch.detected"),
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "total_cost": 0.0,
            "status": "running",
        }
        for event in events:
            attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
            if event.get("event") == "model.usage":
                summary["total_input_tokens"] += int(attrs.get("input_tokens") or 0)
                summary["total_output_tokens"] += int(attrs.get("output_tokens") or 0)
                summary["total_cost"] += float(attrs.get("cost") or 0.0)
            if event.get("event") == "run.finished":
                summary["status"] = str(attrs.get("status") or "completed")
            elif event.get("event") == "run.failed":
                summary["status"] = str(attrs.get("status") or "failed")
        self._write_json(self._summary_path(session_id, run_id), summary)

    def _reconstruct_state_from_transcript(self, session_id: str) -> dict[str, Any] | None:
        transcript_path = self._transcript_path(session_id)
        if not transcript_path.exists():
            return None
        session_record = self._read_json(self._session_json_path(session_id)) or {}
        messages = _read_jsonl(transcript_path)
        return {
            "session_id": session_id,
            "workspace": session_record.get("workspace") or str(Path.cwd()),
            "status": session_record.get("status") or SessionStatus.IDLE.value,
            "messages": messages,
            "todos": [],
            "metadata": {},
        }

    def _session_dir(self, session_id: str) -> Path:
        return self.root / session_id

    def _run_dir(self, session_id: str, run_id: str) -> Path:
        return self._session_dir(session_id) / "runs" / run_id

    def _session_json_path(self, session_id: str) -> Path:
        return self._session_dir(session_id) / "session.json"

    def _transcript_path(self, session_id: str) -> Path:
        return self._session_dir(session_id) / "transcript.jsonl"

    def _state_path(self, session_id: str) -> Path:
        return self._session_dir(session_id) / "state.latest.json"

    def _run_json_path(self, session_id: str, run_id: str) -> Path:
        return self._run_dir(session_id, run_id) / "run.json"

    def _events_path(self, session_id: str, run_id: str) -> Path:
        return self._run_dir(session_id, run_id) / "events.jsonl"

    def _parts_path(self, session_id: str, run_id: str) -> Path:
        return self._run_dir(session_id, run_id) / "parts.jsonl"

    def _context_pack_dir(self, session_id: str, run_id: str) -> Path:
        return self._run_dir(session_id, run_id) / "context"

    def _context_asset_dir(self, session_id: str, run_id: str) -> Path:
        return self._run_dir(session_id, run_id) / "context"

    def _session_memory_path(self, session_id: str) -> Path:
        return self._session_dir(session_id) / "session-memory.md"

    def _summary_path(self, session_id: str, run_id: str) -> Path:
        return self._run_dir(session_id, run_id) / "summary.json"

    def _index_path(self) -> Path:
        return self.root / "index.jsonl"

    def _append_index(self, payload: dict[str, Any]) -> None:
        self._append_jsonl(self._index_path(), payload)

    def _append_jsonl(self, path: Path, payload: dict[str, Any]) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(_jsonable(payload), ensure_ascii=False, sort_keys=True) + "\n")

    def _write_json(self, path: Path, payload: dict[str, Any]) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(path.suffix + ".tmp")
        tmp.write_text(json.dumps(_jsonable(payload), ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        tmp.replace(path)

    def _read_json(self, path: Path) -> dict[str, Any] | None:
        if not path.exists():
            return None
        payload = json.loads(path.read_text(encoding="utf-8"))
        return payload if isinstance(payload, dict) else None

    def _next_seq(self, path: Path) -> int:
        if not path.exists():
            return 1
        return sum(1 for line in path.read_text(encoding="utf-8").splitlines() if line.strip()) + 1


def load_session_store(options: dict[str, Any] | None, *, base_dir: str | Path | None = None) -> SessionStore | None:
    return FileSessionStore.from_options(options, base_dir=base_dir)


def _message_id(message: ChatMessage) -> str:
    metadata = message.metadata
    value = metadata.get("message_id")
    if value:
        return str(value)
    message_id = new_id("msg")
    metadata["message_id"] = message_id
    return message_id


def _message_to_dict(message: ChatMessage, *, index: int) -> dict[str, Any]:
    return {
        "message_id": _message_id(message),
        "index": index,
        "role": message.role,
        "content": message.content,
        "name": message.name,
        "tool_call_id": message.tool_call_id,
        "metadata": _jsonable(message.metadata),
    }


def _message_from_dict(payload: dict[str, Any]) -> ChatMessage:
    return ChatMessage(
        role=payload.get("role", "user"),
        content=str(payload.get("content") or ""),
        name=payload.get("name"),
        tool_call_id=payload.get("tool_call_id"),
        metadata=dict(payload.get("metadata") or {}),
    )


def _read_jsonl(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    rows: list[dict[str, Any]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        item = json.loads(line)
        if isinstance(item, dict):
            rows.append(item)
    return rows


def _count_by_key(rows: list[dict[str, Any]], key: str) -> dict[str, int]:
    counts: dict[str, int] = {}
    for row in rows:
        value = row.get(key)
        if not isinstance(value, str) or not value:
            continue
        counts[value] = counts.get(value, 0) + 1
    return dict(sorted(counts.items()))


def _jsonable(value: Any) -> Any:
    if is_dataclass(value):
        return _jsonable(asdict(value))
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, dict):
        return {str(key): _jsonable(item) for key, item in value.items()}
    if isinstance(value, (list, tuple, set)):
        return [_jsonable(item) for item in value]
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    return str(value)


def _bool_option(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() not in {"0", "false", "no", "off"}
    return bool(value)


def _optional_int(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _status_value(status: Any) -> str:
    return status.value if isinstance(status, SessionStatus) else str(status)


def _now_ms() -> int:
    return int(time.time() * 1000)


__all__ = [
    "DEFAULT_SESSION_STORE_ROOT",
    "FileSessionStore",
    "SESSION_STORE_METADATA_KEY",
    "SessionStore",
    "load_session_store",
]
