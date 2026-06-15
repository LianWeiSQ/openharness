from __future__ import annotations

import json
import hashlib
import time
from pathlib import Path
from typing import Any

from ..context_assets import LAST_CONTEXT_ASSETS_METADATA_KEY, SESSION_MEMORY_METADATA_KEY
from .session import Session
from .store import FileSessionStore, SessionStore, load_session_store


def resume_session(
    session_id: str,
    *,
    options: dict[str, Any] | None = None,
    root_dir: str | Path | None = None,
    base_dir: str | Path | None = None,
) -> Session:
    """Load a persisted session so a caller can continue it with AgentLoop."""

    store = _resume_store(options=options, root_dir=root_dir, base_dir=base_dir)
    session = store.load_session(session_id)
    context_asset_check = validate_resume_context_assets(session)
    session.metadata["session_resume"] = {
        "resumed_at_ms": int(time.time() * 1000),
        "session_id": session.id,
        "store_type": type(store).__name__,
        "context_asset_check": context_asset_check,
    }
    return session


def load_latest_context_pack_snapshot(session: Session) -> dict[str, Any] | None:
    """Read the latest context pack snapshot referenced by session metadata."""

    metadata = session.metadata.get("last_context_pack_snapshot")
    if not isinstance(metadata, dict):
        return None
    path = metadata.get("snapshot_path")
    if not isinstance(path, str) or not path:
        return None
    snapshot_path = Path(path)
    if not snapshot_path.exists():
        return None
    payload = json.loads(snapshot_path.read_text(encoding="utf-8"))
    return payload if isinstance(payload, dict) else None


def load_latest_context_assets_snapshot(session: Session) -> dict[str, Any] | None:
    metadata = session.metadata.get(LAST_CONTEXT_ASSETS_METADATA_KEY)
    if not isinstance(metadata, dict):
        return None
    path = metadata.get("asset_path")
    if not isinstance(path, str) or not path:
        return None
    asset_path = Path(path)
    if not asset_path.exists():
        return None
    payload = json.loads(asset_path.read_text(encoding="utf-8"))
    return payload if isinstance(payload, dict) else None


def load_session_memory(session: Session) -> str | None:
    metadata = session.metadata.get(SESSION_MEMORY_METADATA_KEY)
    if not isinstance(metadata, dict):
        return None
    path = metadata.get("memory_path")
    if not isinstance(path, str) or not path:
        return None
    memory_path = Path(path)
    if not memory_path.exists():
        return None
    return memory_path.read_text(encoding="utf-8")


def validate_resume_context_assets(session: Session) -> dict[str, Any]:
    snapshot = load_latest_context_assets_snapshot(session)
    if snapshot is None:
        return {
            "status": "missing",
            "instruction_changed_count": 0,
            "file_changed_count": 0,
            "issues": ["missing_context_assets_snapshot"],
        }
    instruction_results = _validate_instruction_assets(snapshot)
    file_results = _validate_file_assets(snapshot)
    changed = [
        item
        for item in [*instruction_results, *file_results]
        if item.get("status") not in {"unchanged", "remote_unchecked"}
    ]
    return {
        "status": "changed" if changed else "unchanged",
        "asset_path": session.metadata.get(LAST_CONTEXT_ASSETS_METADATA_KEY, {}).get("asset_path")
        if isinstance(session.metadata.get(LAST_CONTEXT_ASSETS_METADATA_KEY), dict)
        else None,
        "instruction_count": len(instruction_results),
        "instruction_changed_count": sum(1 for item in instruction_results if item.get("status") != "unchanged"),
        "file_count": len(file_results),
        "file_changed_count": sum(1 for item in file_results if item.get("status") not in {"unchanged", "remote_unchecked"}),
        "instructions": instruction_results,
        "files": file_results,
        "issues": [],
    }


def _resume_store(
    *,
    options: dict[str, Any] | None,
    root_dir: str | Path | None,
    base_dir: str | Path | None,
) -> SessionStore:
    if root_dir is not None:
        return FileSessionStore(root_dir)
    store = load_session_store(options or {}, base_dir=base_dir)
    if store is None:
        raise ValueError("Session store is disabled; pass root_dir or enable options['session_store'].")
    return store


def _validate_instruction_assets(snapshot: dict[str, Any]) -> list[dict[str, Any]]:
    instructions = snapshot.get("instructions") if isinstance(snapshot.get("instructions"), dict) else {}
    items = instructions.get("items") if isinstance(instructions.get("items"), list) else []
    results: list[dict[str, Any]] = []
    for item in items:
        if not isinstance(item, dict):
            continue
        path = str(item.get("path") or "")
        expected = str(item.get("content_hash") or "")
        results.append(_validate_hash_path(path=path, expected_hash=expected, label=item.get("display_path")))
    return results


def _validate_file_assets(snapshot: dict[str, Any]) -> list[dict[str, Any]]:
    files = snapshot.get("files") if isinstance(snapshot.get("files"), dict) else {}
    records = files.get("records") if isinstance(files.get("records"), list) else []
    results: list[dict[str, Any]] = []
    for record in records:
        if not isinstance(record, dict):
            continue
        path = str(record.get("absolute_path") or "")
        expected = str(record.get("content_hash") or "")
        results.append(_validate_hash_path(path=path, expected_hash=expected, label=record.get("path")))
    return results


def _validate_hash_path(*, path: str, expected_hash: str, label: Any) -> dict[str, Any]:
    result = {
        "path": path,
        "display_path": str(label or path),
        "expected_hash": expected_hash,
    }
    if not path:
        return {**result, "status": "invalid"}
    if "://" in path:
        return {**result, "status": "remote_unchecked"}
    target = Path(path)
    if not target.exists():
        return {**result, "status": "missing"}
    if not target.is_file():
        return {**result, "status": "not_file"}
    try:
        current_hash = hashlib.sha256(target.read_bytes()).hexdigest()
    except OSError:
        return {**result, "status": "unreadable"}
    return {
        **result,
        "status": "unchanged" if current_hash == expected_hash else "changed",
        "current_hash": current_hash,
    }


__all__ = [
    "load_latest_context_assets_snapshot",
    "load_latest_context_pack_snapshot",
    "load_session_memory",
    "resume_session",
    "validate_resume_context_assets",
]
