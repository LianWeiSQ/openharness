from __future__ import annotations

import hashlib
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import difflib

from ..id import new_id


@dataclass(frozen=True, slots=True)
class SnapshotFile:
    sha256: str
    text: str | None = None


@dataclass(frozen=True, slots=True)
class Snapshot:
    id: str
    root: Path
    files: dict[str, SnapshotFile]


class SnapshotManager:
    def __init__(
        self,
        *,
        max_file_bytes: int = 1_000_000,
        max_text_bytes: int = 200_000,
    ) -> None:
        self._snapshots: dict[str, Snapshot] = {}
        self.max_file_bytes = max_file_bytes
        self.max_text_bytes = max_text_bytes

    def track(self, root: Path) -> str:
        snap_id = new_id("snapshot")
        file_map = self._scan(root)
        self._snapshots[snap_id] = Snapshot(id=snap_id, root=root.resolve(), files=file_map)
        return snap_id

    def patch(self, snapshot_id: str) -> dict[str, Any]:
        snap = self._snapshots.get(snapshot_id)
        if not snap:
            return {"hash": "", "files": []}
        now = self._scan(snap.root)
        before = snap.files
        changed: list[dict[str, Any]] = []
        paths = set(before.keys()) | set(now.keys())
        for rel in sorted(paths):
            a = before.get(rel)
            b = now.get(rel)
            if a and b and a.sha256 == b.sha256:
                continue
            status = "modified"
            if a is None:
                status = "added"
            elif b is None:
                status = "deleted"
            diff = self._diff(rel, a, b)
            changed.append({"path": rel, "status": status, "diff": diff})
        h = hashlib.sha256()
        for item in changed:
            h.update(item["path"].encode("utf-8"))
            h.update(item["status"].encode("utf-8"))
        return {"hash": h.hexdigest(), "files": changed}

    def _scan(self, root: Path) -> dict[str, SnapshotFile]:
        root = root.resolve()
        out: dict[str, SnapshotFile] = {}
        for dirpath, _dirnames, filenames in os.walk(root):
            for fn in filenames:
                path = Path(dirpath) / fn
                try:
                    size = path.stat().st_size
                except OSError:
                    continue
                if size > self.max_file_bytes:
                    continue
                try:
                    data = path.read_bytes()
                except OSError:
                    continue
                sha = hashlib.sha256(data).hexdigest()
                rel = str(path.relative_to(root))
                text: str | None = None
                if size <= self.max_text_bytes:
                    try:
                        text = data.decode("utf-8")
                    except UnicodeDecodeError:
                        text = None
                out[rel] = SnapshotFile(sha256=sha, text=text)
        return out

    @staticmethod
    def _diff(rel: str, before: SnapshotFile | None, after: SnapshotFile | None) -> str:
        a = (before.text if before else None) or ""
        b = (after.text if after else None) or ""
        if not a and not b:
            return ""
        diff = difflib.unified_diff(
            a.splitlines(),
            b.splitlines(),
            fromfile=f"a/{rel}",
            tofile=f"b/{rel}",
            lineterm="",
        )
        return "\n".join(diff)

