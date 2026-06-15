from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any

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
    session.metadata["session_resume"] = {
        "resumed_at_ms": int(time.time() * 1000),
        "session_id": session.id,
        "store_type": type(store).__name__,
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


__all__ = ["load_latest_context_pack_snapshot", "resume_session"]
