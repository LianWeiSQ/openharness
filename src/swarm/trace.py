from __future__ import annotations

"""Agent-agnostic swarm trace lineage."""

import time
from dataclasses import dataclass, field
from typing import Any, Literal
from uuid import uuid4

TraceStatus = Literal["ok", "error", "running"]


@dataclass(frozen=True, slots=True)
class SwarmTraceEvent:
    seq: int
    trace_id: str
    run_id: str
    span_id: str
    parent_span_id: str | None
    name: str
    kind: str
    status: TraceStatus = "ok"
    runner_id: str | None = None
    task_id: str | None = None
    timestamp_ms: int = 0
    duration_ms: int | None = None
    attributes: dict[str, Any] = field(default_factory=dict)

    def as_dict(self) -> dict[str, Any]:
        return {
            "seq": self.seq,
            "trace_id": self.trace_id,
            "run_id": self.run_id,
            "span_id": self.span_id,
            "parent_span_id": self.parent_span_id,
            "name": self.name,
            "kind": self.kind,
            "status": self.status,
            "runner_id": self.runner_id,
            "task_id": self.task_id,
            "timestamp_ms": self.timestamp_ms,
            "duration_ms": self.duration_ms,
            "attributes": dict(self.attributes),
        }


@dataclass(slots=True)
class _OpenSpan:
    span_id: str
    parent_span_id: str | None
    name: str
    kind: str
    runner_id: str | None
    task_id: str | None
    started_ms: int
    attributes: dict[str, Any]


class SwarmTraceRecorder:
    def __init__(self, *, trace_id: str | None = None, run_id: str) -> None:
        self.trace_id = trace_id or _new_id("trace")
        self.run_id = run_id
        self._seq = 0
        self._events: list[SwarmTraceEvent] = []
        self._open_spans: dict[str, _OpenSpan] = {}

    @property
    def events(self) -> list[SwarmTraceEvent]:
        return list(self._events)

    def start_span(
        self,
        *,
        name: str,
        kind: str,
        parent_span_id: str | None = None,
        runner_id: str | None = None,
        task_id: str | None = None,
        attributes: dict[str, Any] | None = None,
    ) -> str:
        span_id = _new_id("span")
        now = _now_ms()
        self._open_spans[span_id] = _OpenSpan(
            span_id=span_id,
            parent_span_id=parent_span_id,
            name=name,
            kind=kind,
            runner_id=runner_id,
            task_id=task_id,
            started_ms=now,
            attributes=dict(attributes or {}),
        )
        self._append(
            span_id=span_id,
            parent_span_id=parent_span_id,
            name=f"{name}.started",
            kind=kind,
            status="running",
            runner_id=runner_id,
            task_id=task_id,
            timestamp_ms=now,
            attributes=attributes,
        )
        return span_id

    def finish_span(
        self,
        span_id: str,
        *,
        status: TraceStatus = "ok",
        attributes: dict[str, Any] | None = None,
    ) -> None:
        span = self._open_spans.pop(span_id, None)
        now = _now_ms()
        if span is None:
            self.record_event(
                name="span.finish_missing",
                kind="trace",
                parent_span_id=None,
                status="error",
                attributes={"span_id": span_id, **dict(attributes or {})},
            )
            return

        merged_attributes = {**span.attributes, **dict(attributes or {})}
        self._append(
            span_id=span.span_id,
            parent_span_id=span.parent_span_id,
            name=f"{span.name}.finished",
            kind=span.kind,
            status=status,
            runner_id=span.runner_id,
            task_id=span.task_id,
            timestamp_ms=now,
            duration_ms=max(now - span.started_ms, 0),
            attributes=merged_attributes,
        )

    def record_event(
        self,
        *,
        name: str,
        kind: str,
        parent_span_id: str | None,
        status: TraceStatus = "ok",
        runner_id: str | None = None,
        task_id: str | None = None,
        attributes: dict[str, Any] | None = None,
    ) -> str:
        span_id = _new_id("event")
        self._append(
            span_id=span_id,
            parent_span_id=parent_span_id,
            name=name,
            kind=kind,
            status=status,
            runner_id=runner_id,
            task_id=task_id,
            timestamp_ms=_now_ms(),
            attributes=attributes,
        )
        return span_id

    def _append(
        self,
        *,
        span_id: str,
        parent_span_id: str | None,
        name: str,
        kind: str,
        status: TraceStatus,
        runner_id: str | None = None,
        task_id: str | None = None,
        timestamp_ms: int,
        duration_ms: int | None = None,
        attributes: dict[str, Any] | None = None,
    ) -> None:
        self._seq += 1
        self._events.append(
            SwarmTraceEvent(
                seq=self._seq,
                trace_id=self.trace_id,
                run_id=self.run_id,
                span_id=span_id,
                parent_span_id=parent_span_id,
                name=name,
                kind=kind,
                status=status,
                runner_id=runner_id,
                task_id=task_id,
                timestamp_ms=timestamp_ms,
                duration_ms=duration_ms,
                attributes=dict(attributes or {}),
            )
        )


def _new_id(prefix: str) -> str:
    return f"{prefix}_{uuid4().hex}"


def _now_ms() -> int:
    return int(time.time() * 1000)


__all__ = ["SwarmTraceEvent", "SwarmTraceRecorder", "TraceStatus"]
