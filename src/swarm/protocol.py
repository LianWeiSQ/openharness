from __future__ import annotations

"""Stable protocol types for the agent-agnostic swarm kernel."""

import asyncio
from collections.abc import AsyncIterator
from dataclasses import dataclass, field
from typing import Any, Literal, Protocol

RunStatus = Literal["completed", "partial", "failed", "cancelled"]
PermissionMode = Literal["READONLY", "FULL", "PLAN_ONLY", "NONE"]


@dataclass(frozen=True, slots=True)
class RunLimits:
    max_steps: int | None = None
    max_input_tokens: int | None = None
    max_output_tokens: int | None = None
    max_cost: float | None = None
    timeout_seconds: float | None = None


@dataclass(frozen=True, slots=True)
class FanoutBudget:
    max_concurrent: int = 4
    max_total_workers: int = 8
    max_total_tokens: int | None = None
    max_total_cost: float | None = None

    def normalized(self) -> "FanoutBudget":
        return FanoutBudget(
            max_concurrent=max(1, int(self.max_concurrent)),
            max_total_workers=max(1, int(self.max_total_workers)),
            max_total_tokens=self.max_total_tokens,
            max_total_cost=self.max_total_cost,
        )


@dataclass(frozen=True, slots=True)
class Usage:
    input_tokens: int = 0
    output_tokens: int = 0
    cost: float = 0.0
    steps: int = 0
    latency_ms: int = 0

    @property
    def total_tokens(self) -> int:
        return int(self.input_tokens) + int(self.output_tokens)

    def plus(self, other: "Usage") -> "Usage":
        return Usage(
            input_tokens=self.input_tokens + other.input_tokens,
            output_tokens=self.output_tokens + other.output_tokens,
            cost=self.cost + other.cost,
            steps=self.steps + other.steps,
            latency_ms=self.latency_ms + other.latency_ms,
        )


@dataclass(frozen=True, slots=True)
class ArtifactRef:
    kind: str
    uri: str
    title: str = ""
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class AgentSpec:
    role: str
    objective: str
    context: str
    boundaries: str
    output_schema: dict[str, Any]
    inputs: dict[str, Any] = field(default_factory=dict)
    limits: RunLimits = field(default_factory=RunLimits)
    permissions: PermissionMode = "READONLY"
    metadata: dict[str, Any] = field(default_factory=dict)

    def validate(self) -> None:
        missing: list[str] = []
        if not self.role.strip():
            missing.append("role")
        if not self.objective.strip():
            missing.append("objective")
        if not self.context.strip():
            missing.append("context")
        if not self.boundaries.strip():
            missing.append("boundaries")
        if not isinstance(self.output_schema, dict) or not self.output_schema:
            missing.append("output_schema")
        if missing:
            raise ValueError(f"AgentSpec is missing required contract fields: {', '.join(missing)}")


@dataclass(frozen=True, slots=True)
class AgentResult:
    status: RunStatus
    summary: str
    evidence: list[str] = field(default_factory=list)
    open_questions: list[str] = field(default_factory=list)
    confidence: float = 0.0
    artifacts: list[ArtifactRef] = field(default_factory=list)
    usage: Usage = field(default_factory=Usage)
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class AgentDescriptor:
    id: str
    roles: list[str]
    tool_groups: list[str] = field(default_factory=list)
    model_tier: str = "worker"
    max_context: int = 0
    supports_streaming: bool = False
    kind: str = "function"
    metadata: dict[str, Any] = field(default_factory=dict)

    def supports_role(self, role: str) -> bool:
        return role in self.roles or "*" in self.roles


@dataclass(frozen=True, slots=True)
class RunContext:
    run_id: str
    parent_span_id: str | None = None
    budget: FanoutBudget = field(default_factory=FanoutBudget)
    cancellation: asyncio.Event | None = None
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class AgentEvent:
    type: str
    run_id: str
    runner_id: str
    message: str = ""
    metadata: dict[str, Any] = field(default_factory=dict)


class AgentRunHandle(Protocol):
    def events(self) -> AsyncIterator[AgentEvent]: ...

    async def result(self) -> AgentResult: ...

    async def cancel(self) -> None: ...


class AgentRunner(Protocol):
    @property
    def descriptor(self) -> AgentDescriptor: ...

    async def start(self, spec: AgentSpec, ctx: RunContext) -> AgentRunHandle: ...


def usage_from_mapping(value: dict[str, Any] | None) -> Usage:
    if not value:
        return Usage()
    return Usage(
        input_tokens=int(value.get("input_tokens") or 0),
        output_tokens=int(value.get("output_tokens") or 0),
        cost=float(value.get("cost") or 0.0),
        steps=int(value.get("steps") or 0),
        latency_ms=int(value.get("latency_ms") or 0),
    )
