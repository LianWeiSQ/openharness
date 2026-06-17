from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Any

from openagent.core.types import StreamEvent


@dataclass(slots=True)
class AppEvent:
    """UI-facing event inspired by Codex app-server thread/turn/item events."""

    sequence: int
    method: str
    params: dict[str, Any]
    created_at_ms: int = field(default_factory=lambda: int(time.time() * 1000))

    def to_dict(self) -> dict[str, Any]:
        return {
            "sequence": self.sequence,
            "method": self.method,
            "params": self.params,
            "created_at_ms": self.created_at_ms,
        }


def stream_event_to_app_method(event_type: str) -> str:
    if event_type == "step-start":
        return "item/step/started"
    if event_type == "step-finish":
        return "item/step/completed"
    if event_type == "text-start":
        return "item/agentMessage/started"
    if event_type == "text-delta":
        return "item/agentMessage/delta"
    if event_type == "text-end":
        return "item/agentMessage/completed"
    if event_type == "tool-call":
        return "item/toolCall/started"
    if event_type == "tool-result":
        return "item/toolCall/completed"
    if event_type == "runtime-warning":
        return "runtime/warning"
    if event_type == "patch":
        return "item/patch/detected"
    if event_type == "question-request":
        return "item/question/requested"
    if event_type == "error":
        return "turn/error"
    return "item/event"


def stream_event_to_app_event(
    event: StreamEvent,
    *,
    sequence: int,
    thread_id: str,
    turn_id: str,
) -> AppEvent:
    event_type = str(event.get("type") or "unknown")
    return AppEvent(
        sequence=sequence,
        method=stream_event_to_app_method(event_type),
        params={
            "thread_id": thread_id,
            "turn_id": turn_id,
            "source": "openagent",
            "event_type": event_type,
            "event": _json_safe(dict(event)),
        },
    )


def lifecycle_event(
    *,
    sequence: int,
    method: str,
    thread_id: str,
    turn_id: str | None = None,
    **params: Any,
) -> AppEvent:
    payload = {"thread_id": thread_id, **params}
    if turn_id is not None:
        payload["turn_id"] = turn_id
    return AppEvent(sequence=sequence, method=method, params=_json_safe(payload))


def _json_safe(value: Any) -> Any:
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, list):
        return [_json_safe(item) for item in value]
    if isinstance(value, tuple):
        return [_json_safe(item) for item in value]
    if isinstance(value, dict):
        return {str(key): _json_safe(item) for key, item in value.items()}
    return str(value)
