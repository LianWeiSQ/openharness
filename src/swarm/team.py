from __future__ import annotations

"""Team handoff manifests for resumable swarm runs."""

import json
from dataclasses import asdict, dataclass, field, replace
from pathlib import Path
from typing import Any

from .config import TaskConfig
from .protocol import AgentResult

DEFAULT_TEAM_REUSE_STATUSES = ("completed",)


@dataclass(frozen=True, slots=True)
class TeamRunnerHandoff:
    runner_id: str
    status: str
    reusable: bool
    summary: str = ""
    evidence: list[str] = field(default_factory=list)
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class TeamHandoff:
    run_id: str
    task_id: str
    role: str
    status: str
    runner_ids: list[str]
    reusable_runner_ids: list[str]
    pending_runner_ids: list[str]
    failed_runner_ids: list[str] = field(default_factory=list)
    partial_runner_ids: list[str] = field(default_factory=list)
    cancelled_runner_ids: list[str] = field(default_factory=list)
    missing_runner_ids: list[str] = field(default_factory=list)
    resume_reuse_statuses: tuple[str, ...] = DEFAULT_TEAM_REUSE_STATUSES
    warnings: list[str] = field(default_factory=list)
    task_contract: dict[str, Any] = field(default_factory=dict)
    runners: list[TeamRunnerHandoff] = field(default_factory=list)
    schema_version: int = 1

    @property
    def has_pending(self) -> bool:
        return bool(self.pending_runner_ids)

    def as_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "run_id": self.run_id,
            "task_id": self.task_id,
            "role": self.role,
            "status": self.status,
            "runner_ids": list(self.runner_ids),
            "reusable_runner_ids": list(self.reusable_runner_ids),
            "pending_runner_ids": list(self.pending_runner_ids),
            "failed_runner_ids": list(self.failed_runner_ids),
            "partial_runner_ids": list(self.partial_runner_ids),
            "cancelled_runner_ids": list(self.cancelled_runner_ids),
            "missing_runner_ids": list(self.missing_runner_ids),
            "resume_reuse_statuses": list(self.resume_reuse_statuses),
            "warnings": list(self.warnings),
            "task_contract": _json_safe(self.task_contract),
            "runners": [_json_safe(asdict(runner)) for runner in self.runners],
        }


class FileTeamHandoffStore:
    def __init__(self, root: str | Path) -> None:
        self.root = Path(root).resolve()

    def run_dir(self, run_id: str) -> Path:
        return self.root / _safe_name(run_id)

    def handoff_path(self, run_id: str) -> Path:
        return self.run_dir(run_id) / "team-handoff.json"

    def save_handoff(self, handoff: TeamHandoff) -> dict[str, Any]:
        payload = handoff.as_dict()
        run_dir = self.run_dir(handoff.run_id)
        run_dir.mkdir(parents=True, exist_ok=True)
        self.handoff_path(handoff.run_id).write_text(
            json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return payload

    def load_handoff(self, run_id: str) -> TeamHandoff:
        with self.handoff_path(run_id).open("r", encoding="utf-8") as handle:
            payload = json.load(handle)
        if not isinstance(payload, dict):
            raise ValueError("team handoff payload must be a mapping")
        return team_handoff_from_dict(payload)


def build_team_handoff(
    *,
    task: TaskConfig,
    result: Any,
    run_id: str,
    resume_reuse_statuses: tuple[str, ...] = DEFAULT_TEAM_REUSE_STATUSES,
) -> TeamHandoff:
    results = dict(getattr(result, "results", {}) or {})
    runner_ids = list(task.runner_ids) or list(results)
    reusable: list[str] = []
    pending: list[str] = []
    failed: list[str] = []
    partial: list[str] = []
    cancelled: list[str] = []
    missing: list[str] = []
    runners: list[TeamRunnerHandoff] = []

    for runner_id in runner_ids:
        agent_result = results.get(runner_id)
        if not isinstance(agent_result, AgentResult):
            missing.append(runner_id)
            pending.append(runner_id)
            runners.append(TeamRunnerHandoff(runner_id=runner_id, status="missing", reusable=False))
            continue
        is_reusable = agent_result.status in set(resume_reuse_statuses)
        if is_reusable:
            reusable.append(runner_id)
        else:
            pending.append(runner_id)
        if agent_result.status == "failed":
            failed.append(runner_id)
        elif agent_result.status == "partial":
            partial.append(runner_id)
        elif agent_result.status == "cancelled":
            cancelled.append(runner_id)
        runners.append(
            TeamRunnerHandoff(
                runner_id=runner_id,
                status=agent_result.status,
                reusable=is_reusable,
                summary=agent_result.summary,
                evidence=list(agent_result.evidence),
                metadata=_json_safe(dict(agent_result.metadata or {})),
            )
        )

    return TeamHandoff(
        run_id=run_id,
        task_id=task.id,
        role=task.role,
        status=str(getattr(result, "status", "")),
        runner_ids=runner_ids,
        reusable_runner_ids=reusable,
        pending_runner_ids=pending,
        failed_runner_ids=failed,
        partial_runner_ids=partial,
        cancelled_runner_ids=cancelled,
        missing_runner_ids=missing,
        resume_reuse_statuses=tuple(resume_reuse_statuses),
        warnings=[str(item) for item in getattr(result, "warnings", [])],
        task_contract=_task_contract(task),
        runners=runners,
    )


def team_handoff_from_dict(payload: dict[str, Any]) -> TeamHandoff:
    return TeamHandoff(
        schema_version=int(payload.get("schema_version") or 1),
        run_id=str(payload.get("run_id") or ""),
        task_id=str(payload.get("task_id") or ""),
        role=str(payload.get("role") or ""),
        status=str(payload.get("status") or ""),
        runner_ids=[str(item) for item in payload.get("runner_ids") or []],
        reusable_runner_ids=[str(item) for item in payload.get("reusable_runner_ids") or []],
        pending_runner_ids=[str(item) for item in payload.get("pending_runner_ids") or []],
        failed_runner_ids=[str(item) for item in payload.get("failed_runner_ids") or []],
        partial_runner_ids=[str(item) for item in payload.get("partial_runner_ids") or []],
        cancelled_runner_ids=[str(item) for item in payload.get("cancelled_runner_ids") or []],
        missing_runner_ids=[str(item) for item in payload.get("missing_runner_ids") or []],
        resume_reuse_statuses=tuple(str(item) for item in payload.get("resume_reuse_statuses") or DEFAULT_TEAM_REUSE_STATUSES),
        warnings=[str(item) for item in payload.get("warnings") or []],
        task_contract=dict(payload.get("task_contract") or {}),
        runners=[
            TeamRunnerHandoff(
                runner_id=str(item.get("runner_id") or ""),
                status=str(item.get("status") or ""),
                reusable=bool(item.get("reusable")),
                summary=str(item.get("summary") or ""),
                evidence=[str(entry) for entry in item.get("evidence") or []],
                metadata=dict(item.get("metadata") or {}),
            )
            for item in payload.get("runners") or []
            if isinstance(item, dict)
        ],
    )


def task_for_team_handoff_resume(
    *,
    task: TaskConfig,
    handoff: TeamHandoff,
    pending_only: bool = False,
) -> TaskConfig:
    runner_ids = list(handoff.pending_runner_ids) if pending_only else list(handoff.runner_ids)
    metadata = dict(task.metadata)
    metadata["resume"] = {
        **dict(metadata.get("resume") if isinstance(metadata.get("resume"), dict) else {}),
        "enabled": True,
        "reuse_statuses": list(handoff.resume_reuse_statuses),
    }
    metadata["team_handoff"] = {
        "run_id": handoff.run_id,
        "task_id": handoff.task_id,
        "pending_runner_ids": list(handoff.pending_runner_ids),
        "reusable_runner_ids": list(handoff.reusable_runner_ids),
    }
    return replace(task, runner_ids=runner_ids, metadata=metadata)


def _task_contract(task: TaskConfig) -> dict[str, Any]:
    return {
        "id": task.id,
        "role": task.role,
        "objective": task.objective,
        "context": task.context,
        "boundaries": task.boundaries,
        "output_schema": _json_safe(task.output_schema),
        "inputs": _json_safe(task.inputs),
        "permissions": task.permissions,
        "runner_ids": list(task.runner_ids),
    }


def _json_safe(value: Any) -> Any:
    try:
        json.dumps(value)
        return value
    except TypeError:
        if isinstance(value, dict):
            return {str(key): _json_safe(item) for key, item in value.items()}
        if isinstance(value, (list, tuple, set)):
            return [_json_safe(item) for item in value]
        return str(value)


def _safe_name(value: str) -> str:
    cleaned = "".join(char if char.isalnum() or char in {"-", "_"} else "_" for char in value).strip("_")
    return cleaned or "run"


__all__ = [
    "DEFAULT_TEAM_REUSE_STATUSES",
    "FileTeamHandoffStore",
    "TeamHandoff",
    "TeamRunnerHandoff",
    "build_team_handoff",
    "task_for_team_handoff_resume",
    "team_handoff_from_dict",
]
