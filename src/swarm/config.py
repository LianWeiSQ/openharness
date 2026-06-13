from __future__ import annotations

"""YAML configuration for swarm/function mode."""

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

from .protocol import AgentDescriptor, FanoutBudget, PermissionMode, RunLimits


@dataclass(frozen=True, slots=True)
class RunnerConfig:
    id: str
    kind: str = "function"
    roles: list[str] = field(default_factory=lambda: ["*"])
    handler: str | None = None
    tool_groups: list[str] = field(default_factory=list)
    model_tier: str = "worker"
    max_context: int = 0
    supports_streaming: bool = False
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_descriptor(self) -> AgentDescriptor:
        return AgentDescriptor(
            id=self.id,
            roles=list(self.roles),
            tool_groups=list(self.tool_groups),
            model_tier=self.model_tier,
            max_context=int(self.max_context),
            supports_streaming=bool(self.supports_streaming),
            kind=self.kind,
            metadata=dict(self.metadata),
        )


@dataclass(frozen=True, slots=True)
class TaskConfig:
    id: str
    objective: str
    context: str
    boundaries: str
    output_schema: dict[str, Any]
    role: str = "worker"
    runner_ids: list[str] = field(default_factory=list)
    inputs: dict[str, Any] = field(default_factory=dict)
    limits: RunLimits = field(default_factory=RunLimits)
    permissions: PermissionMode = "READONLY"
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class SwarmConfig:
    runners: list[RunnerConfig] = field(default_factory=list)
    tasks: list[TaskConfig] = field(default_factory=list)
    fanout_budget: FanoutBudget = field(default_factory=FanoutBudget)

    def task(self, task_id: str) -> TaskConfig:
        for task in self.tasks:
            if task.id == task_id:
                return task
        raise KeyError(f'task "{task_id}" is not configured')


def load_swarm_config(source: str | Path | dict[str, Any]) -> SwarmConfig:
    if isinstance(source, dict):
        payload = source
    else:
        raw = Path(source)
        text = raw.read_text(encoding="utf-8") if raw.exists() else str(source)
        loaded = yaml.safe_load(text) or {}
        if not isinstance(loaded, dict):
            raise ValueError("swarm config must be a mapping")
        payload = loaded

    return SwarmConfig(
        runners=_parse_runners(payload.get("runners") or {}),
        tasks=_parse_tasks(payload.get("tasks") or {}),
        fanout_budget=_parse_budget(payload.get("fanout_budget") or payload.get("budget") or {}),
    )


def _parse_runners(value: Any) -> list[RunnerConfig]:
    if isinstance(value, list):
        items = value
    elif isinstance(value, dict):
        items = [dict(raw or {}, id=runner_id) for runner_id, raw in value.items()]
    else:
        raise ValueError("runners must be a mapping or list")

    runners: list[RunnerConfig] = []
    for raw in items:
        if not isinstance(raw, dict):
            raise ValueError("each runner must be a mapping")
        runner_id = str(raw.get("id") or "").strip()
        if not runner_id:
            raise ValueError("runner id is required")
        runners.append(
            RunnerConfig(
                id=runner_id,
                kind=str(raw.get("kind") or "function"),
                roles=[str(item) for item in raw.get("roles") or ["*"]],
                handler=str(raw.get("handler")).strip() if raw.get("handler") else None,
                tool_groups=[str(item) for item in raw.get("tool_groups") or []],
                model_tier=str(raw.get("model_tier") or "worker"),
                max_context=int(raw.get("max_context") or 0),
                supports_streaming=bool(raw.get("supports_streaming") or False),
                metadata=dict(raw.get("metadata") or {}),
            )
        )
    return runners


def _parse_tasks(value: Any) -> list[TaskConfig]:
    if isinstance(value, list):
        items = value
    elif isinstance(value, dict):
        items = [dict(raw or {}, id=task_id) for task_id, raw in value.items()]
    else:
        raise ValueError("tasks must be a mapping or list")

    tasks: list[TaskConfig] = []
    for raw in items:
        if not isinstance(raw, dict):
            raise ValueError("each task must be a mapping")
        task_id = str(raw.get("id") or "").strip()
        if not task_id:
            raise ValueError("task id is required")
        tasks.append(
            TaskConfig(
                id=task_id,
                role=str(raw.get("role") or "worker"),
                objective=str(raw.get("objective") or ""),
                context=str(raw.get("context") or ""),
                boundaries=str(raw.get("boundaries") or ""),
                output_schema=dict(raw.get("output_schema") or {}),
                runner_ids=[str(item) for item in raw.get("runner_ids") or raw.get("runners") or []],
                inputs=dict(raw.get("inputs") or {}),
                limits=_parse_limits(raw.get("limits") or {}),
                permissions=str(raw.get("permissions") or "READONLY"),  # type: ignore[arg-type]
                metadata=dict(raw.get("metadata") or {}),
            )
        )
    return tasks


def _parse_limits(value: dict[str, Any]) -> RunLimits:
    return RunLimits(
        max_steps=_optional_int(value.get("max_steps")),
        max_input_tokens=_optional_int(value.get("max_input_tokens")),
        max_output_tokens=_optional_int(value.get("max_output_tokens")),
        max_cost=_optional_float(value.get("max_cost")),
        timeout_seconds=_optional_float(value.get("timeout_seconds")),
    )


def _parse_budget(value: dict[str, Any]) -> FanoutBudget:
    return FanoutBudget(
        max_concurrent=int(value.get("max_concurrent") or 4),
        max_total_workers=int(value.get("max_total_workers") or 8),
        max_total_tokens=_optional_int(value.get("max_total_tokens")),
        max_total_cost=_optional_float(value.get("max_total_cost")),
    ).normalized()


def _optional_int(value: Any) -> int | None:
    if value is None:
        return None
    return int(value)


def _optional_float(value: Any) -> float | None:
    if value is None:
        return None
    return float(value)
