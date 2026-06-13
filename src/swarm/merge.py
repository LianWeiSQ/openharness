from __future__ import annotations

"""Merge-back review helpers for isolated swarm worker outputs."""

import hashlib
import shutil
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Literal

from .protocol import AgentResult

MergeChangeType = Literal["added", "modified", "deleted"]


@dataclass(frozen=True, slots=True)
class MergeChange:
    runner_id: str
    relative_path: str
    change_type: MergeChangeType
    workspace_path: str | None = None
    source_path: str | None = None
    content_hash: str | None = None
    size_bytes: int = 0
    conflict: bool = False


@dataclass(frozen=True, slots=True)
class MergeConflict:
    relative_path: str
    runner_ids: list[str]
    reason: str
    change_types: list[str] = field(default_factory=list)


@dataclass(frozen=True, slots=True)
class MergePlan:
    source_root: str
    changes: list[MergeChange] = field(default_factory=list)
    conflicts: list[MergeConflict] = field(default_factory=list)

    @property
    def has_conflicts(self) -> bool:
        return bool(self.conflicts)

    @property
    def conflict_paths(self) -> set[str]:
        return {conflict.relative_path for conflict in self.conflicts}

    @property
    def non_conflicting_changes(self) -> list[MergeChange]:
        conflicts = self.conflict_paths
        selected: dict[str, MergeChange] = {}
        for change in self.changes:
            if change.relative_path in conflicts:
                continue
            selected.setdefault(change.relative_path, change)
        return list(selected.values())


@dataclass(frozen=True, slots=True)
class MergeApplyResult:
    target_root: str
    applied: list[MergeChange] = field(default_factory=list)
    skipped_conflicts: list[str] = field(default_factory=list)


def build_merge_plan(
    results: dict[str, AgentResult],
    *,
    source_root: str | Path | None = None,
) -> MergePlan:
    resolved_source = _resolve_source_root(results, source_root)
    changes: list[MergeChange] = []
    for runner_id, result in results.items():
        workspace = result.metadata.get("worker_workspace")
        if not workspace:
            continue
        workspace_root = Path(str(workspace)).resolve()
        if not workspace_root.exists():
            continue
        changes.extend(_scan_worker_changes(runner_id=runner_id, source_root=resolved_source, workspace_root=workspace_root))

    conflicts = _detect_conflicts(changes)
    conflict_paths = {conflict.relative_path for conflict in conflicts}
    marked_changes = [replace(change, conflict=change.relative_path in conflict_paths) for change in changes]
    return MergePlan(source_root=str(resolved_source), changes=marked_changes, conflicts=conflicts)


def apply_merge_plan(
    plan: MergePlan,
    *,
    target_root: str | Path | None = None,
    include_conflicts: bool = False,
) -> MergeApplyResult:
    target = Path(target_root or plan.source_root).resolve()
    target.mkdir(parents=True, exist_ok=True)
    conflict_paths = plan.conflict_paths
    applied: list[MergeChange] = []
    skipped_conflicts: list[str] = []
    selected: dict[str, MergeChange] = {}
    for change in plan.changes:
        if change.relative_path in conflict_paths and not include_conflicts:
            if change.relative_path not in skipped_conflicts:
                skipped_conflicts.append(change.relative_path)
            continue
        selected.setdefault(change.relative_path, change)

    for change in selected.values():
        target_path = target / change.relative_path
        if change.change_type == "deleted":
            if target_path.exists() and target_path.is_file():
                target_path.unlink()
            applied.append(change)
            continue
        if not change.workspace_path:
            continue
        source_path = Path(change.workspace_path)
        if not source_path.exists() or not source_path.is_file():
            continue
        target_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source_path, target_path)
        applied.append(change)
    return MergeApplyResult(target_root=str(target), applied=applied, skipped_conflicts=skipped_conflicts)


def _resolve_source_root(results: dict[str, AgentResult], source_root: str | Path | None) -> Path:
    if source_root is not None:
        return Path(source_root).resolve()
    roots = {
        str(Path(str(result.metadata["workspace_source_root"])).resolve())
        for result in results.values()
        if result.metadata.get("workspace_source_root")
    }
    if len(roots) > 1:
        raise ValueError("merge plan requires a single source_root; pass source_root explicitly for mixed worker sources")
    if len(roots) == 1:
        return Path(next(iter(roots))).resolve()
    for result in results.values():
        value = result.metadata.get("workspace_source_root")
        if value:
            return Path(str(value)).resolve()
    raise ValueError("source_root is required when results do not include workspace_source_root metadata")


def _scan_worker_changes(*, runner_id: str, source_root: Path, workspace_root: Path) -> list[MergeChange]:
    source_files = _file_map(source_root)
    workspace_files = _file_map(workspace_root)
    relative_paths = sorted(set(source_files) | set(workspace_files))
    changes: list[MergeChange] = []
    for relative_path in relative_paths:
        source_path = source_files.get(relative_path)
        workspace_path = workspace_files.get(relative_path)
        if source_path is None and workspace_path is not None:
            changes.append(_change(runner_id=runner_id, relative_path=relative_path, change_type="added", workspace_path=workspace_path))
            continue
        if source_path is not None and workspace_path is None:
            changes.append(
                MergeChange(
                    runner_id=runner_id,
                    relative_path=relative_path,
                    change_type="deleted",
                    source_path=str(source_path),
                )
            )
            continue
        if source_path is None or workspace_path is None:
            continue
        source_hash = _hash_file(source_path)
        workspace_hash = _hash_file(workspace_path)
        if source_hash != workspace_hash:
            changes.append(
                _change(
                    runner_id=runner_id,
                    relative_path=relative_path,
                    change_type="modified",
                    workspace_path=workspace_path,
                    source_path=source_path,
                )
            )
    return changes


def _detect_conflicts(changes: list[MergeChange]) -> list[MergeConflict]:
    by_path: dict[str, list[MergeChange]] = {}
    for change in changes:
        by_path.setdefault(change.relative_path, []).append(change)

    conflicts: list[MergeConflict] = []
    for relative_path, path_changes in sorted(by_path.items()):
        if len(path_changes) < 2:
            continue
        signatures = {_change_signature(change) for change in path_changes}
        if len(signatures) <= 1:
            continue
        conflicts.append(
            MergeConflict(
                relative_path=relative_path,
                runner_ids=[change.runner_id for change in path_changes],
                reason="multiple workers changed the same path differently",
                change_types=sorted({change.change_type for change in path_changes}),
            )
        )
    return conflicts


def _change(
    *,
    runner_id: str,
    relative_path: str,
    change_type: MergeChangeType,
    workspace_path: Path,
    source_path: Path | None = None,
) -> MergeChange:
    return MergeChange(
        runner_id=runner_id,
        relative_path=relative_path,
        change_type=change_type,
        workspace_path=str(workspace_path),
        source_path=str(source_path) if source_path else None,
        content_hash=_hash_file(workspace_path),
        size_bytes=workspace_path.stat().st_size,
    )


def _change_signature(change: MergeChange) -> tuple[str, str | None]:
    if change.change_type == "deleted":
        return ("deleted", None)
    return (change.change_type, change.content_hash)


def _file_map(root: Path) -> dict[str, Path]:
    if not root.exists():
        return {}
    if root.is_file():
        return {root.name: root}
    files: dict[str, Path] = {}
    for path in root.rglob("*"):
        if path.is_file():
            files[str(path.relative_to(root))] = path
    return files


def _hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


__all__ = [
    "MergeApplyResult",
    "MergeChange",
    "MergeChangeType",
    "MergeConflict",
    "MergePlan",
    "apply_merge_plan",
    "build_merge_plan",
]
