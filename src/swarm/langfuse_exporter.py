from __future__ import annotations

"""Optional Langfuse export for swarm trace lineage."""

import hashlib
import os
from dataclasses import dataclass, field
from typing import Any

from .trace import SwarmTraceEvent

CONTENT_KEY_MARKERS = (
    "arguments",
    "completion",
    "context",
    "input",
    "message",
    "objective",
    "output",
    "prompt",
    "result",
    "summary",
    "text",
)
SAFE_KEYS = {
    "confidence",
    "cost",
    "duration_ms",
    "http_status",
    "input_tokens",
    "latency_ms",
    "output_tokens",
    "response_format",
    "returncode",
    "runner_count",
    "status",
    "steps",
    "summary_chars",
    "total_tokens",
    "transport",
}


@dataclass(frozen=True, slots=True)
class SwarmLangfuseExportResult:
    enabled: bool
    trace_id: str | None = None
    observations_sent: int = 0
    diagnostics: list[dict[str, Any]] = field(default_factory=list)


class SwarmLangfuseExporter:
    """Maps SDK-free swarm trace events into Langfuse observations."""

    name = "langfuse"

    def __init__(
        self,
        *,
        client: Any,
        langfuse_trace_id: str,
        include_content: bool = False,
        environment: str = "local",
        tags: list[str] | None = None,
    ) -> None:
        self.client = client
        self.langfuse_trace_id = langfuse_trace_id
        self.include_content = include_content
        self.environment = environment
        self.tags = list(tags or ["openagent", "swarm"])
        self._observations: dict[str, Any] = {}
        self._observation_ids: dict[str, str] = {}
        self._closed = False
        self._sent = 0

    @classmethod
    def from_config(cls, *, trace_seed: str, options: dict[str, Any]) -> "SwarmLangfuseExporter":
        client = load_langfuse_client(options)
        return cls(
            client=client,
            langfuse_trace_id=_langfuse_trace_id(client, trace_seed),
            include_content=_bool_option(options.get("include_content", False)),
            environment=str(options.get("environment") or os.getenv("LANGFUSE_ENVIRONMENT") or "local"),
            tags=_string_list(options.get("tags"), default=["openagent", "swarm"]),
        )

    def export(self, events: list[SwarmTraceEvent]) -> SwarmLangfuseExportResult:
        if self._closed:
            return SwarmLangfuseExportResult(enabled=True, trace_id=self.langfuse_trace_id, observations_sent=self._sent)
        for event in sorted(events, key=lambda item: item.seq):
            self.record_event(event)
        self.close()
        return SwarmLangfuseExportResult(enabled=True, trace_id=self.langfuse_trace_id, observations_sent=self._sent)

    def record_event(self, event: SwarmTraceEvent) -> None:
        if self._closed:
            return
        if _is_swarm_span_start(event):
            self._start_span(event)
            return
        if _is_swarm_span_finish(event):
            self._finish_span(event)
            return
        self._record_instant_observation(event)

    def close(self) -> None:
        if self._closed:
            return
        for key in sorted(self._observations, reverse=True):
            observation = self._observations.pop(key)
            _safe_end(observation)
        flush = getattr(self.client, "flush", None)
        if callable(flush):
            flush()
        self._closed = True

    def metadata(self) -> dict[str, Any]:
        return {
            "enabled": True,
            "trace_id": self.langfuse_trace_id,
            "include_content": self.include_content,
            "environment": self.environment,
            "tags": list(self.tags),
            "observations_sent": self._sent,
        }

    def _start_span(self, event: SwarmTraceEvent) -> None:
        key = _event_key(event)
        if key in self._observations:
            return
        observation = self._start_client_observation(event)
        self._update_observation(observation, event, terminal=False)
        self._observations[key] = observation
        observation_id = _observation_id(observation)
        if observation_id:
            self._observation_ids[key] = observation_id
        self._sent += 1

    def _finish_span(self, event: SwarmTraceEvent) -> None:
        key = _event_key(event)
        observation = self._observations.pop(key, None)
        if observation is None:
            observation = self._start_client_observation(event)
            self._sent += 1
        self._update_observation(observation, event, terminal=True)
        _safe_end(observation)

    def _record_instant_observation(self, event: SwarmTraceEvent) -> None:
        observation = self._start_client_observation(event)
        self._update_observation(observation, event, terminal=True)
        _safe_end(observation)
        self._sent += 1

    def _start_client_observation(self, event: SwarmTraceEvent) -> Any:
        method = getattr(self.client, "start_observation", None)
        if not callable(method):
            raise RuntimeError("Langfuse client does not support start_observation().")
        trace_context = {"trace_id": self.langfuse_trace_id}
        parent_id = self._observation_ids.get(_parent_key(event))
        if parent_id:
            trace_context["parent_span_id"] = parent_id
        try:
            return method(name=_observation_name(event), as_type=_observation_type(event), trace_context=trace_context)
        except TypeError:
            return method(name=_observation_name(event), as_type=_observation_type(event))

    def _update_observation(self, observation: Any, event: SwarmTraceEvent, *, terminal: bool) -> None:
        payload: dict[str, Any] = {"metadata": self._metadata_payload(event)}
        if terminal:
            payload["level"] = "ERROR" if event.status == "error" else "DEFAULT"
            if event.status == "error":
                payload["status_message"] = _status_message(event.attributes)
        usage = _usage_details(event.attributes)
        if usage:
            payload["usage_details"] = usage
        cost = _float_value(event.attributes.get("cost"))
        if cost is not None:
            payload["cost_details"] = {"total": cost}
        _safe_update(observation, payload)

    def _metadata_payload(self, event: SwarmTraceEvent) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "swarm_event": event.name,
            "swarm_seq": event.seq,
            "swarm_status": event.status,
            "swarm_kind": event.kind,
            "swarm_run_id": event.run_id,
            "swarm_trace_id": event.trace_id,
            "langfuse_environment": self.environment,
            "langfuse_tags": list(self.tags),
        }
        if event.runner_id:
            payload["swarm_runner_id"] = event.runner_id
        if event.task_id:
            payload["swarm_task_id"] = event.task_id
        if event.duration_ms is not None:
            payload["swarm_duration_ms"] = event.duration_ms
        for key, value in event.attributes.items():
            if _should_export_attribute(str(key), include_content=self.include_content):
                payload[f"attr_{_normalize_attr_key(str(key))}"] = value
        return payload


def export_swarm_trace_to_langfuse(
    events: list[SwarmTraceEvent],
    *,
    options: dict[str, Any] | None = None,
) -> SwarmLangfuseExportResult:
    options = dict(options or {})
    if not _bool_option(options.get("enabled", False)):
        return SwarmLangfuseExportResult(enabled=False)
    if not events:
        return SwarmLangfuseExportResult(enabled=True, observations_sent=0)
    diagnostics: list[dict[str, Any]] = []
    try:
        exporter = SwarmLangfuseExporter.from_config(trace_seed=events[0].trace_id, options=options)
        result = exporter.export(events)
        return SwarmLangfuseExportResult(
            enabled=True,
            trace_id=result.trace_id,
            observations_sent=result.observations_sent,
            diagnostics=diagnostics,
        )
    except Exception as error:  # noqa: BLE001
        diagnostic = {
            "exporter": "langfuse",
            "status": "error",
            "error_kind": type(error).__name__,
            "message": str(error),
        }
        diagnostics.append(diagnostic)
        if _bool_option(options.get("strict", False)):
            raise
        return SwarmLangfuseExportResult(enabled=True, diagnostics=diagnostics)


def load_langfuse_client(options: dict[str, Any] | None) -> Any:
    options = dict(options or {})
    try:
        from langfuse import Langfuse, get_client
    except ImportError as error:
        raise RuntimeError("Langfuse exporter requires optional dependencies: pip install 'openagent-core[langfuse]'") from error

    public_key_env = str(options.get("public_key_env") or "LANGFUSE_PUBLIC_KEY")
    secret_key_env = str(options.get("secret_key_env") or "LANGFUSE_SECRET_KEY")
    base_url_env = str(options.get("base_url_env") or "LANGFUSE_BASE_URL")
    public_key = str(options.get("public_key") or os.getenv(public_key_env) or "").strip()
    secret_key = str(options.get("secret_key") or os.getenv(secret_key_env) or "").strip()
    base_url = str(options.get("base_url") or os.getenv(base_url_env) or "").strip()
    keys_required = _bool_option(options.get("keys_required", True))
    if keys_required and (not public_key or not secret_key):
        raise ValueError(f"Langfuse exporter is enabled but {public_key_env} or {secret_key_env} is missing.")
    if public_key or secret_key or base_url:
        kwargs: dict[str, str] = {}
        if public_key:
            kwargs["public_key"] = public_key
        if secret_key:
            kwargs["secret_key"] = secret_key
        if base_url:
            kwargs["base_url"] = base_url
        return Langfuse(**kwargs)
    return get_client()


def _is_swarm_span_start(event: SwarmTraceEvent) -> bool:
    return event.name.startswith("swarm.") and event.name.endswith(".started")


def _is_swarm_span_finish(event: SwarmTraceEvent) -> bool:
    return event.name.startswith("swarm.") and event.name.endswith(".finished")


def _event_key(event: SwarmTraceEvent) -> str:
    return f"span:{event.span_id}"


def _parent_key(event: SwarmTraceEvent) -> str:
    return f"span:{event.parent_span_id}" if event.parent_span_id else ""


def _observation_name(event: SwarmTraceEvent) -> str:
    if event.name.startswith("swarm.runner") and event.runner_id:
        return f"swarm.runner {event.runner_id}"
    if event.name.startswith("swarm.task") and event.task_id:
        return f"swarm.task {event.task_id}"
    if event.name.startswith("swarm.run"):
        return f"swarm.run {event.run_id}"
    return event.name


def _observation_type(event: SwarmTraceEvent) -> str:
    if event.kind == "run":
        return "agent"
    return "span"


def _observation_id(observation: Any) -> str:
    return str(getattr(observation, "id", "") or getattr(observation, "observation_id", "") or "")


def _should_export_attribute(key: str, *, include_content: bool) -> bool:
    if include_content:
        return True
    if key in SAFE_KEYS:
        return True
    lowered = key.lower()
    return not any(marker in lowered for marker in CONTENT_KEY_MARKERS)


def _normalize_attr_key(key: str) -> str:
    return "".join(char if char.isalnum() else "_" for char in key).strip("_") or "value"


def _status_message(attributes: dict[str, Any]) -> str | None:
    if attributes.get("error_kind"):
        return str(attributes["error_kind"])
    if attributes.get("error"):
        return str(attributes["error"])
    if attributes.get("status"):
        return str(attributes["status"])
    return None


def _usage_details(attrs: dict[str, Any]) -> dict[str, int]:
    usage: dict[str, int] = {}
    input_tokens = _int_value(attrs.get("input_tokens"))
    output_tokens = _int_value(attrs.get("output_tokens"))
    steps = _int_value(attrs.get("steps"))
    if input_tokens is not None:
        usage["input_tokens"] = input_tokens
    if output_tokens is not None:
        usage["output_tokens"] = output_tokens
    if steps is not None:
        usage["steps"] = steps
    if input_tokens is not None or output_tokens is not None:
        usage["total_tokens"] = int(input_tokens or 0) + int(output_tokens or 0)
    return usage


def _int_value(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _float_value(value: Any) -> float | None:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _bool_option(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "on", "true", "yes"}
    return bool(value)


def _string_list(value: Any, *, default: list[str]) -> list[str]:
    if isinstance(value, str):
        items = [value]
    elif isinstance(value, list):
        items = value
    elif isinstance(value, tuple):
        items = list(value)
    else:
        items = default
    normalized = [str(item).strip() for item in items if str(item).strip()]
    return normalized or list(default)


def _langfuse_trace_id(client: Any, seed: str) -> str:
    create_trace_id = getattr(client, "create_trace_id", None)
    if callable(create_trace_id):
        try:
            value = str(create_trace_id(seed=seed))
            if _is_hex(value, 32):
                return value
        except Exception:  # noqa: BLE001
            pass
    return hashlib.md5(seed.encode("utf-8")).hexdigest()  # noqa: S324


def _is_hex(value: str, length: int) -> bool:
    if len(value) != length:
        return False
    return all(char in "0123456789abcdef" for char in value)


def _safe_update(observation: Any, payload: dict[str, Any]) -> None:
    update = getattr(observation, "update", None)
    if not callable(update):
        return
    cleaned = {key: value for key, value in payload.items() if value is not None}
    try:
        update(**cleaned)
    except TypeError:
        update(cleaned)


def _safe_end(observation: Any) -> None:
    end = getattr(observation, "end", None)
    if callable(end):
        end()


__all__ = [
    "SwarmLangfuseExportResult",
    "SwarmLangfuseExporter",
    "export_swarm_trace_to_langfuse",
    "load_langfuse_client",
]
