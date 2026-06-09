from __future__ import annotations

import json
import time
import traceback
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Iterator

from .id import new_id
from .trace import AgentTraceRecorder

OBSERVABILITY_METADATA_KEY = "observability"
DEFAULT_JSONL_DIR = ".openagent/observability"
DEFAULT_MAX_EVENTS = 500
DEFAULT_INPUT_PREVIEW_CHARS = 2048
DEFAULT_FIELD_PREVIEW_CHARS = 4096
SENSITIVE_KEY_MARKERS = (
    "api_key",
    "apikey",
    "authorization",
    "cookie",
    "password",
    "secret",
    "token",
)
SAFE_TOKEN_METRIC_KEYS = {
    "estimated_input_tokens",
    "input_limit_tokens",
    "input_tokens",
    "max_output_tokens",
    "output_tokens",
    "reserved_output_tokens",
}


@dataclass(frozen=True, slots=True)
class ObservationConfig:
    enabled: bool = True
    keep_events: bool = True
    jsonl: bool = False
    jsonl_dir: str = DEFAULT_JSONL_DIR
    max_events: int = DEFAULT_MAX_EVENTS
    input_preview_chars: int = DEFAULT_INPUT_PREVIEW_CHARS
    include_traceback: bool = False


@dataclass(frozen=True, slots=True)
class TraceRecord:
    trace_id: str
    session_id: str
    run_id: str
    agent_name: str
    model_id: str | None = None
    provider_id: str | None = None
    workspace: str | None = None
    started_at_ms: int = 0


@dataclass(frozen=True, slots=True)
class ObservationEvent:
    event_id: str
    trace_id: str
    run_id: str
    session_id: str
    span_id: str | None
    parent_span_id: str | None
    name: str
    kind: str
    timestamp_ms: int
    duration_ms: int | None = None
    status: str = "ok"
    attributes: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(slots=True)
class SpanContext:
    recorder: "ObservationRecorder"
    span_id: str
    parent_span_id: str | None
    name: str
    kind: str
    started_at_ms: int
    attributes: dict[str, Any] = field(default_factory=dict)
    status: str = "ok"
    error: dict[str, Any] | None = None

    def set_attribute(self, key: str, value: Any) -> None:
        self.attributes[key] = value

    def set_attributes(self, values: dict[str, Any]) -> None:
        self.attributes.update(values)

    def record_error(self, error: BaseException, *, error_kind: str | None = None) -> None:
        self.status = "error"
        self.error = self.recorder.error_payload(error, error_kind=error_kind)


class ObservationRecorder:
    def __init__(
        self,
        *,
        trace: TraceRecord,
        session_metadata: dict[str, Any],
        config: ObservationConfig | None = None,
        base_dir: Path | str | None = None,
        trace_recorder: AgentTraceRecorder | None = None,
    ) -> None:
        self.trace = trace
        self.session_metadata = session_metadata
        self.config = config or ObservationConfig()
        self.base_dir = Path(base_dir) if base_dir is not None else Path.cwd()
        self.trace_recorder = trace_recorder
        self._span_stack: ContextVar[tuple[str, ...]] = ContextVar(
            f"openagent_observation_span_stack_{trace.trace_id}",
            default=(),
        )
        if self.config.enabled:
            self._ensure_metadata_root()

    @classmethod
    def for_session(
        cls,
        *,
        session_id: str,
        session_metadata: dict[str, Any],
        agent_name: str,
        model_id: str | None,
        provider_id: str | None,
        workspace: str | None,
        options: dict[str, Any] | None,
        base_dir: Path | str | None = None,
    ) -> "ObservationRecorder":
        config = load_observation_config(options)
        run_id = new_id("run")
        trace = TraceRecord(
            trace_id=new_id("trace"),
            session_id=session_id,
            run_id=run_id,
            agent_name=agent_name,
            model_id=model_id,
            provider_id=provider_id,
            workspace=workspace,
            started_at_ms=_now_ms(),
        )
        trace_recorder = (
            AgentTraceRecorder.for_observation_trace(
                trace=trace,
                options=options,
                base_dir=base_dir,
                session_metadata=session_metadata,
            )
            if config.enabled
            else None
        )
        return cls(
            trace=trace,
            session_metadata=session_metadata,
            config=config,
            base_dir=base_dir,
            trace_recorder=trace_recorder,
        )

    @property
    def current_span_id(self) -> str | None:
        stack = self._span_stack.get()
        return stack[-1] if stack else None

    def event(
        self,
        name: str,
        *,
        kind: str = "event",
        status: str = "ok",
        attributes: dict[str, Any] | None = None,
        span_id: str | None = None,
        parent_span_id: str | None = None,
        duration_ms: int | None = None,
    ) -> ObservationEvent | None:
        if not self.config.enabled:
            return None
        event = ObservationEvent(
            event_id=new_id("event"),
            trace_id=self.trace.trace_id,
            run_id=self.trace.run_id,
            session_id=self.trace.session_id,
            span_id=span_id if span_id is not None else self.current_span_id,
            parent_span_id=parent_span_id,
            name=name,
            kind=kind,
            timestamp_ms=_now_ms(),
            duration_ms=duration_ms,
            status=status,
            attributes=sanitize_observation_value(attributes or {}),
        )
        self._record(event)
        return event

    @contextmanager
    def span(
        self,
        name: str,
        *,
        kind: str,
        attributes: dict[str, Any] | None = None,
    ) -> Iterator[SpanContext]:
        span_id = new_id("span")
        parent_span_id = self.current_span_id
        started_at_ms = _now_ms()
        ctx = SpanContext(
            recorder=self,
            span_id=span_id,
            parent_span_id=parent_span_id,
            name=name,
            kind=kind,
            started_at_ms=started_at_ms,
            attributes=dict(attributes or {}),
        )
        token = self._span_stack.set((*self._span_stack.get(), span_id))
        self.event(
            f"{name}.started",
            kind=kind,
            attributes=ctx.attributes,
            span_id=span_id,
            parent_span_id=parent_span_id,
        )
        try:
            yield ctx
        except Exception as error:
            ctx.record_error(error)
            raise
        finally:
            self._span_stack.reset(token)
            duration_ms = max(0, _now_ms() - started_at_ms)
            attributes_payload = dict(ctx.attributes)
            if ctx.error:
                attributes_payload["error"] = ctx.error
            self.event(
                f"{name}.finished" if ctx.status == "ok" else f"{name}.failed",
                kind=kind,
                status=ctx.status,
                attributes=attributes_payload,
                span_id=span_id,
                parent_span_id=parent_span_id,
                duration_ms=duration_ms,
            )

    def error_payload(self, error: BaseException, *, error_kind: str | None = None) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "type": type(error).__name__,
            "message": str(error),
        }
        if error_kind:
            payload["error_kind"] = error_kind
        if self.config.include_traceback:
            payload["traceback"] = "".join(traceback.format_exception(type(error), error, error.__traceback__))
        return sanitize_observation_value(payload)

    def _ensure_metadata_root(self) -> None:
        if not self.config.enabled:
            return
        root = self.session_metadata.get(OBSERVABILITY_METADATA_KEY)
        if not isinstance(root, dict):
            root = {}
            self.session_metadata[OBSERVABILITY_METADATA_KEY] = root
        root["trace"] = sanitize_observation_value(asdict(self.trace))
        root.setdefault("events", [])
        root.setdefault("event_count", 0)
        root["jsonl_path"] = str(self._jsonl_path()) if self.config.jsonl else None

    def _record(self, event: ObservationEvent) -> None:
        root = self.session_metadata.get(OBSERVABILITY_METADATA_KEY)
        if not isinstance(root, dict):
            root = {}
            self.session_metadata[OBSERVABILITY_METADATA_KEY] = root
        event_dict = event.to_dict()
        root["event_count"] = int(root.get("event_count") or 0) + 1
        root["last_event_at_ms"] = event.timestamp_ms
        if self.config.keep_events:
            events_raw = root.get("events")
            events = list(events_raw) if isinstance(events_raw, list) else []
            events.append(event_dict)
            root["events"] = events[-max(1, self.config.max_events):]
        if self.config.jsonl:
            self._append_jsonl(event_dict)
        if self.trace_recorder is not None:
            self.trace_recorder.record_observation(event_dict)

    def _append_jsonl(self, event: dict[str, Any]) -> None:
        path = self._jsonl_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(event, ensure_ascii=False, sort_keys=True) + "\n")

    def _jsonl_path(self) -> Path:
        root = Path(self.config.jsonl_dir)
        if not root.is_absolute():
            root = self.base_dir / root
        return root / self.trace.session_id / f"{self.trace.run_id}.jsonl"


def load_observation_config(options: dict[str, Any] | None) -> ObservationConfig:
    raw_options = options or {}
    raw = raw_options.get("observability", {})
    if raw is None:
        raw = {}
    if not isinstance(raw, dict):
        raw = {}
    return ObservationConfig(
        enabled=_bool_option(raw.get("enabled", True)),
        keep_events=_bool_option(raw.get("keep_events", True)),
        jsonl=_bool_option(raw.get("jsonl", False)),
        jsonl_dir=str(raw.get("jsonl_dir") or DEFAULT_JSONL_DIR),
        max_events=_positive_int(raw.get("max_events"), DEFAULT_MAX_EVENTS),
        input_preview_chars=_positive_int(raw.get("input_preview_chars"), DEFAULT_INPUT_PREVIEW_CHARS),
        include_traceback=_bool_option(raw.get("include_traceback", False)),
    )


def sanitize_observation_value(value: Any, *, max_chars: int = DEFAULT_FIELD_PREVIEW_CHARS) -> Any:
    if isinstance(value, dict):
        sanitized: dict[str, Any] = {}
        for key, item in value.items():
            key_text = str(key)
            if _is_sensitive_key(key_text) and key_text not in SAFE_TOKEN_METRIC_KEYS:
                sanitized[key_text] = "[redacted]"
            else:
                sanitized[key_text] = sanitize_observation_value(item, max_chars=max_chars)
        return sanitized
    if isinstance(value, list):
        return [sanitize_observation_value(item, max_chars=max_chars) for item in value]
    if isinstance(value, tuple):
        return [sanitize_observation_value(item, max_chars=max_chars) for item in value]
    if isinstance(value, set):
        return sorted((sanitize_observation_value(item, max_chars=max_chars) for item in value), key=repr)
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, (str, int, float, bool)) or value is None:
        return _truncate_text(value, max_chars) if isinstance(value, str) else value
    return _truncate_text(repr(value), max_chars)


def input_preview(value: Any, *, max_chars: int = DEFAULT_INPUT_PREVIEW_CHARS) -> str:
    sanitized = sanitize_observation_value(value, max_chars=max_chars)
    try:
        rendered = json.dumps(sanitized, ensure_ascii=False, sort_keys=True)
    except TypeError:
        rendered = repr(sanitized)
    return _truncate_text(rendered, max_chars)


def output_stats(output: str | None) -> dict[str, int]:
    text = output or ""
    return {
        "output_bytes": len(text.encode("utf-8")),
        "output_lines": len(text.splitlines()),
    }


def _is_sensitive_key(key: str) -> bool:
    lowered = key.lower()
    return any(marker in lowered for marker in SENSITIVE_KEY_MARKERS)


def _truncate_text(value: str, max_chars: int) -> str:
    if max_chars <= 0:
        return ""
    if len(value) <= max_chars:
        return value
    suffix = f"...[truncated {len(value) - max(0, max_chars - 24)} chars]"
    return value[: max(0, max_chars - len(suffix))] + suffix


def _bool_option(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "on", "true", "yes"}
    return bool(value)


def _positive_int(value: Any, default: int) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        return default
    return parsed if parsed > 0 else default


def _now_ms() -> int:
    return int(time.time() * 1000)


__all__ = [
    "OBSERVABILITY_METADATA_KEY",
    "ObservationConfig",
    "ObservationEvent",
    "ObservationRecorder",
    "TraceRecord",
    "input_preview",
    "load_observation_config",
    "output_stats",
    "sanitize_observation_value",
]
