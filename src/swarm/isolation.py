from __future__ import annotations

"""Workspace isolation helpers for swarm workers."""

import shutil
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal
from uuid import uuid4

IsolationMode = Literal["copy", "empty"]


@dataclass(frozen=True, slots=True)
class WorkerWorkspaceConfig:
    enabled: bool = False
    mode: IsolationMode = "copy"
    source_root: str | None = None
    base_dir: str | None = None
    cleanup: bool = False
    exclude: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class WorkerWorkspace:
    path: str
    mode: IsolationMode
    source_root: str | None = None
    cleanup: bool = False

    def as_metadata(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "workspace_isolated": True,
            "workspace_mode": self.mode,
            "worker_workspace": self.path,
            "workspace_cleanup": self.cleanup,
        }
        if self.source_root:
            payload["workspace_source_root"] = self.source_root
        return payload

    def cleanup_path(self) -> None:
        if self.cleanup:
            shutil.rmtree(self.path, ignore_errors=True)


def resolve_worker_workspace_config(
    *,
    task_metadata: dict[str, Any],
    runner_metadata: dict[str, Any],
) -> WorkerWorkspaceConfig:
    merged: dict[str, Any] = {}
    task_raw = _raw_isolation(task_metadata)
    runner_raw = _raw_isolation(runner_metadata)
    if task_raw is not None:
        merged.update(_raw_to_mapping(task_raw))
    if runner_raw is not None:
        merged.update(_raw_to_mapping(runner_raw))
    if not merged:
        return WorkerWorkspaceConfig()

    enabled = _bool_option(merged.get("enabled", True))
    mode = str(merged.get("mode") or "copy").strip().lower()
    if mode not in {"copy", "empty"}:
        raise ValueError(f'unsupported workspace isolation mode "{mode}"')
    exclude = merged.get("exclude") or ()
    if isinstance(exclude, str):
        exclude_items = (exclude,)
    elif isinstance(exclude, (list, tuple)):
        exclude_items = tuple(str(item) for item in exclude)
    else:
        raise ValueError("workspace isolation exclude must be a string or list")
    return WorkerWorkspaceConfig(
        enabled=enabled,
        mode=mode,  # type: ignore[arg-type]
        source_root=str(merged.get("source_root") or merged.get("source") or "").strip() or None,
        base_dir=str(merged.get("base_dir") or merged.get("root") or "").strip() or None,
        cleanup=_bool_option(merged.get("cleanup", False)),
        exclude=exclude_items,
    )


def prepare_worker_workspace(
    *,
    run_id: str,
    task_id: str,
    runner_id: str,
    config: WorkerWorkspaceConfig,
) -> WorkerWorkspace | None:
    if not config.enabled:
        return None
    base_dir = Path(config.base_dir or Path(tempfile.gettempdir()) / "openagent-swarm-workspaces").resolve()
    target = base_dir / _safe_name(run_id) / _safe_name(task_id) / f"{_safe_name(runner_id)}-{uuid4().hex[:8]}"
    if config.mode == "empty":
        target.mkdir(parents=True, exist_ok=False)
    else:
        if not config.source_root:
            raise ValueError("workspace isolation mode copy requires source_root")
        source = Path(config.source_root).resolve()
        if not source.exists():
            raise ValueError(f'workspace isolation source_root does not exist: {source}')
        if source.is_dir():
            shutil.copytree(
                source,
                target,
                ignore=shutil.ignore_patterns(*config.exclude) if config.exclude else None,
            )
        else:
            target.mkdir(parents=True, exist_ok=False)
            shutil.copy2(source, target / source.name)
    return WorkerWorkspace(
        path=str(target),
        mode=config.mode,
        source_root=str(Path(config.source_root).resolve()) if config.mode == "copy" and config.source_root else None,
        cleanup=config.cleanup,
    )


def _raw_isolation(metadata: dict[str, Any]) -> Any | None:
    if "isolation" in metadata:
        return metadata["isolation"]
    if "workspace_isolation" in metadata:
        return metadata["workspace_isolation"]
    return None


def _raw_to_mapping(value: Any) -> dict[str, Any]:
    if isinstance(value, bool):
        return {"enabled": value}
    if isinstance(value, str):
        return {"enabled": True, "mode": value}
    if isinstance(value, dict):
        return dict(value)
    raise ValueError("workspace isolation metadata must be a bool, string, or mapping")


def _bool_option(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "on", "true", "yes"}
    return bool(value)


def _safe_name(value: str) -> str:
    cleaned = "".join(char if char.isalnum() or char in {"-", "_"} else "_" for char in value).strip("_")
    return cleaned or "worker"


__all__ = [
    "IsolationMode",
    "WorkerWorkspace",
    "WorkerWorkspaceConfig",
    "prepare_worker_workspace",
    "resolve_worker_workspace_config",
]
