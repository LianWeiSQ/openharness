from __future__ import annotations

"""Helpers for normalizing runner return values."""

from typing import Any

from .protocol import AgentResult, Usage, usage_from_mapping

RunnerResultPayload = AgentResult | str | dict[str, Any]


def normalize_result_payload(value: RunnerResultPayload) -> AgentResult:
    if isinstance(value, AgentResult):
        return value
    if isinstance(value, str):
        return AgentResult(status="completed", summary=value)
    if isinstance(value, dict):
        return AgentResult(
            status=str(value.get("status") or "completed"),  # type: ignore[arg-type]
            summary=str(value.get("summary") or value),
            evidence=[str(item) for item in value.get("evidence") or []],
            open_questions=[str(item) for item in value.get("open_questions") or []],
            confidence=float(value.get("confidence") or 0.0),
            usage=_usage_from_value(value.get("usage")),
            metadata=dict(value.get("metadata") or {}),
        )
    return AgentResult(status="completed", summary=str(value))


def _usage_from_value(value: Any) -> Usage:
    return usage_from_mapping(value if isinstance(value, dict) else None)


__all__ = ["RunnerResultPayload", "normalize_result_payload"]
