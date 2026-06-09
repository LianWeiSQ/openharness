from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Any, Literal

TraceStatus = Literal["ok", "error"]


@dataclass(frozen=True, slots=True)
class TraceConfig:
    enabled: bool = True
    root_dir: str = ".openagent/runs"
    keep_events: bool = True
    max_events: int = 2000
    write_summary: bool = True
    exporters: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class RunRecord:
    run_id: str
    trace_id: str
    session_id: str
    agent_name: str
    model_id: str | None = None
    provider_id: str | None = None
    workspace: str | None = None
    started_at_ms: int = 0

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True, slots=True)
class TraceEvent:
    seq: int
    event: str
    timestamp_ms: int
    run_id: str
    trace_id: str
    session_id: str
    event_id: str | None = None
    kind: str = "event"
    status: TraceStatus = "ok"
    span_id: str | None = None
    parent_span_id: str | None = None
    duration_ms: int | None = None
    attributes: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True, slots=True)
class StepRecord:
    step_id: str
    run_id: str
    type: str
    name: str
    status: TraceStatus = "ok"
    started_at_ms: int | None = None
    ended_at_ms: int | None = None
    latency_ms: int | None = None
    attributes: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True, slots=True)
class ModelCallRecord:
    run_id: str
    provider: str | None
    model: str | None
    input_tokens: int = 0
    output_tokens: int = 0
    latency_ms: int | None = None
    cost: float = 0.0
    finish_reason: str | None = None
    status: TraceStatus = "ok"
    attributes: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True, slots=True)
class ToolCallRecord:
    run_id: str
    tool_name: str
    source: str
    call_id: str | None = None
    status: TraceStatus = "ok"
    latency_ms: int | None = None
    arguments_preview: str | None = None
    result_summary: str | None = None
    error_kind: str | None = None
    attributes: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True, slots=True)
class ArtifactRecord:
    artifact_id: str
    run_id: str
    kind: str
    path: str | None = None
    title: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True, slots=True)
class ErrorRecord:
    run_id: str
    message: str
    error_kind: str = "error"
    step_index: int | None = None
    attempt_index: int | None = None
    attributes: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


__all__ = [
    "ArtifactRecord",
    "ErrorRecord",
    "ModelCallRecord",
    "RunRecord",
    "StepRecord",
    "ToolCallRecord",
    "TraceConfig",
    "TraceEvent",
    "TraceStatus",
]
