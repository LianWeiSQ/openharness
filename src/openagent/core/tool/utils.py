from __future__ import annotations

import os
from pathlib import Path
from typing import Any


def session_root_from_ctx(ctx: dict[str, Any]) -> Path:
    """Resolve session root from tool context, defaulting to cwd."""

    root_value = ctx.get("session_root") or os.getcwd()
    return Path(str(root_value)).resolve()


def ensure_within_root(root: Path, target: Path) -> Path:
    """Ensure target path is within root, returning the resolved target."""

    root_resolved = root.resolve()
    target_resolved = target.resolve()
    if root_resolved not in target_resolved.parents and root_resolved != target_resolved:
        raise PermissionError(f"Path escapes session root: {target}")
    return target_resolved


def resolve_path_in_root(root: Path, path: str) -> Path:
    """Resolve a required path under root (relative paths are joined to root)."""

    target = Path(path)
    if not target.is_absolute():
        target = root / target
    return ensure_within_root(root, target)


def resolve_optional_path(root: Path, path: str | None) -> Path:
    """Resolve an optional path under root; defaults to root when path is None."""

    target = Path(path) if path else root
    if not target.is_absolute():
        target = root / target
    return ensure_within_root(root, target)
