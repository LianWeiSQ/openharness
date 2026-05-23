from __future__ import annotations

import hashlib
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from .context_pack import ContextItem

FILE_CONTEXT_METADATA_KEY = "file_context_state"
DEFAULT_PREVIEW_CHARS = 2000
DEFAULT_MAX_CONTEXT_ITEMS = 20


@dataclass(frozen=True, slots=True)
class FileContextRecord:
    path: str
    absolute_path: str
    mtime_ns: int
    size_bytes: int
    content_hash: str
    read_at_ms: int
    preview: str
    source_tool: str

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "FileContextRecord | None":
        try:
            return cls(
                path=str(payload["path"]),
                absolute_path=str(payload["absolute_path"]),
                mtime_ns=int(payload["mtime_ns"]),
                size_bytes=int(payload["size_bytes"]),
                content_hash=str(payload["content_hash"]),
                read_at_ms=int(payload["read_at_ms"]),
                preview=str(payload.get("preview") or ""),
                source_tool=str(payload.get("source_tool") or "unknown"),
            )
        except (KeyError, TypeError, ValueError):
            return None

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True, slots=True)
class FileContextChange:
    record: FileContextRecord
    exists: bool
    changed: bool
    reason: str


class FileContextState:
    def __init__(self, records: dict[str, FileContextRecord] | None = None) -> None:
        self.records = dict(records or {})

    @classmethod
    def from_metadata(cls, metadata: dict[str, Any]) -> "FileContextState":
        raw = metadata.get(FILE_CONTEXT_METADATA_KEY)
        if not isinstance(raw, dict):
            return cls()
        raw_records = raw.get("records")
        if not isinstance(raw_records, dict):
            return cls()
        records: dict[str, FileContextRecord] = {}
        for key, value in raw_records.items():
            if not isinstance(value, dict):
                continue
            record = FileContextRecord.from_dict(value)
            if record is not None:
                records[str(key)] = record
        return cls(records)

    def to_metadata(self) -> dict[str, Any]:
        return {
            "schema_version": 1,
            "records": {
                key: record.to_dict()
                for key, record in sorted(self.records.items())
            },
        }

    def write_to_metadata(self, metadata: dict[str, Any]) -> None:
        metadata[FILE_CONTEXT_METADATA_KEY] = self.to_metadata()

    def record_read(
        self,
        path: str | Path,
        *,
        workspace_root: str | Path | None = None,
        content: str | bytes | None = None,
        source_tool: str = "read",
        now_ms: int | None = None,
        preview_chars: int = DEFAULT_PREVIEW_CHARS,
    ) -> FileContextRecord:
        absolute = _resolve_path(path, workspace_root=workspace_root)
        raw = _content_bytes(absolute, content)
        stat = absolute.stat()
        display_path = _display_path(absolute, workspace_root=workspace_root)
        text_preview = _preview_text(raw, max_chars=preview_chars)
        record = FileContextRecord(
            path=display_path,
            absolute_path=str(absolute),
            mtime_ns=int(stat.st_mtime_ns),
            size_bytes=int(stat.st_size),
            content_hash=_sha256(raw),
            read_at_ms=now_ms if now_ms is not None else int(time.time() * 1000),
            preview=text_preview,
            source_tool=source_tool,
        )
        self.records[record.absolute_path] = record
        return record

    def change_for(self, path: str | Path) -> FileContextChange | None:
        raw_path = str(path)
        record = self.records.get(raw_path)
        if record is not None and "://" in record.absolute_path:
            return FileContextChange(record=record, exists=True, changed=False, reason="remote_unchecked")
        absolute = Path(path).expanduser().resolve()
        record = record or self.records.get(str(absolute))
        if record is None:
            return None
        if not absolute.exists():
            return FileContextChange(record=record, exists=False, changed=True, reason="missing")
        stat = absolute.stat()
        if int(stat.st_size) != record.size_bytes:
            return FileContextChange(record=record, exists=True, changed=True, reason="size")
        if int(stat.st_mtime_ns) != record.mtime_ns:
            try:
                current_hash = _sha256(absolute.read_bytes())
            except OSError:
                return FileContextChange(record=record, exists=False, changed=True, reason="unreadable")
            if current_hash != record.content_hash:
                return FileContextChange(record=record, exists=True, changed=True, reason="hash")
        return FileContextChange(record=record, exists=True, changed=False, reason="unchanged")

    def changed_records(self) -> list[FileContextChange]:
        changes: list[FileContextChange] = []
        for record in self.records.values():
            change = self.change_for(record.absolute_path)
            if change is not None and change.changed:
                changes.append(change)
        return changes

    def to_context_items(self, *, max_items: int = DEFAULT_MAX_CONTEXT_ITEMS) -> list[ContextItem]:
        records = sorted(self.records.values(), key=lambda item: item.read_at_ms, reverse=True)[:max_items]
        return [self._record_to_context_item(record) for record in records]

    @staticmethod
    def _record_to_context_item(record: FileContextRecord) -> ContextItem:
        digest = hashlib.sha1(record.absolute_path.encode("utf-8")).hexdigest()[:12]
        content = "\n".join(
            [
                f"[File context] {record.path}",
                f"source_tool={record.source_tool}",
                f"size_bytes={record.size_bytes}",
                f"content_hash={record.content_hash}",
                "preview:",
                record.preview,
            ]
        ).strip()
        return ContextItem(
            id=f"file:{digest}",
            kind="file",
            source="file_context_state",
            content=content,
            priority=70,
            metadata={
                "path": record.path,
                "absolute_path": record.absolute_path,
                "mtime_ns": record.mtime_ns,
                "size_bytes": record.size_bytes,
                "content_hash": record.content_hash,
                "read_at_ms": record.read_at_ms,
                "source_tool": record.source_tool,
            },
        )


def record_file_read(
    metadata: dict[str, Any],
    path: str | Path,
    *,
    workspace_root: str | Path | None = None,
    content: str | bytes | None = None,
    source_tool: str = "read",
    now_ms: int | None = None,
    preview_chars: int = DEFAULT_PREVIEW_CHARS,
) -> FileContextRecord:
    state = FileContextState.from_metadata(metadata)
    record = state.record_read(
        path,
        workspace_root=workspace_root,
        content=content,
        source_tool=source_tool,
        now_ms=now_ms,
        preview_chars=preview_chars,
    )
    state.write_to_metadata(metadata)
    return record


def record_virtual_file_read(
    metadata: dict[str, Any],
    *,
    absolute_path: str,
    display_path: str,
    content: str | bytes,
    source_tool: str = "read",
    now_ms: int | None = None,
    preview_chars: int = DEFAULT_PREVIEW_CHARS,
) -> FileContextRecord:
    state = FileContextState.from_metadata(metadata)
    raw = _content_bytes_from_value(content)
    record = FileContextRecord(
        path=display_path,
        absolute_path=absolute_path,
        mtime_ns=0,
        size_bytes=len(raw),
        content_hash=_sha256(raw),
        read_at_ms=now_ms if now_ms is not None else int(time.time() * 1000),
        preview=_preview_text(raw, max_chars=preview_chars),
        source_tool=source_tool,
    )
    state.records[record.absolute_path] = record
    state.write_to_metadata(metadata)
    return record


def _resolve_path(path: str | Path, *, workspace_root: str | Path | None) -> Path:
    candidate = Path(path).expanduser()
    if candidate.is_absolute() or workspace_root is None:
        return candidate.resolve()
    root = Path(workspace_root).expanduser().resolve()
    cwd_resolved = (Path.cwd() / candidate).resolve()
    try:
        cwd_resolved.relative_to(root)
        return cwd_resolved
    except ValueError:
        return (root / candidate).resolve()


def _display_path(path: Path, *, workspace_root: str | Path | None) -> str:
    if workspace_root is not None:
        root = Path(workspace_root).expanduser().resolve()
        try:
            return path.relative_to(root).as_posix()
        except ValueError:
            pass
    return str(path)


def _content_bytes(path: Path, content: str | bytes | None) -> bytes:
    if isinstance(content, bytes):
        return content
    if isinstance(content, str):
        return content.encode("utf-8")
    return path.read_bytes()


def _content_bytes_from_value(content: str | bytes) -> bytes:
    if isinstance(content, bytes):
        return content
    return content.encode("utf-8")


def _preview_text(raw: bytes, *, max_chars: int) -> str:
    text = raw.decode("utf-8", errors="replace")
    if max_chars <= 0:
        return ""
    return text[:max_chars]


def _sha256(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


__all__ = [
    "DEFAULT_MAX_CONTEXT_ITEMS",
    "DEFAULT_PREVIEW_CHARS",
    "FILE_CONTEXT_METADATA_KEY",
    "FileContextChange",
    "FileContextRecord",
    "FileContextState",
    "record_file_read",
    "record_virtual_file_read",
]
