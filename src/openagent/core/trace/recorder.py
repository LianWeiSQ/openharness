from __future__ import annotations

import json
import time
from dataclasses import asdict, is_dataclass
from pathlib import Path
from typing import Any

from ..id import new_id
from .schema import (
    ArtifactRecord,
    ErrorRecord,
    ModelCallRecord,
    RunRecord,
    StepRecord,
    ToolCallRecord,
    TraceConfig,
    TraceEvent,
)

TRACE_METADATA_KEY = "agent_trace"
DEFAULT_TRACE_ROOT = ".openagent/runs"
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


class AgentTraceRecorder:
    """Standard run trace writer for Agent Runtime events."""

    def __init__(
        self,
        *,
        run: RunRecord,
        config: TraceConfig | None = None,
        base_dir: str | Path | None = None,
        session_metadata: dict[str, Any] | None = None,
    ) -> None:
        self.run = run
        self.config = config or TraceConfig()
        self.base_dir = Path(base_dir) if base_dir is not None else Path.cwd()
        self.session_metadata = session_metadata
        self._seq = 0
        self._events: list[dict[str, Any]] = []
        self._summary: dict[str, Any] = self._empty_summary()
        if self.config.enabled:
            self.run_dir.mkdir(parents=True, exist_ok=True)
            self.artifacts_dir.mkdir(parents=True, exist_ok=True)
            self._bind_metadata()
            self._write_process_note("Trace recorder initialized.")
            self._write_summary()

    @classmethod
    def for_observation_trace(
        cls,
        *,
        trace: Any,
        options: dict[str, Any] | None,
        base_dir: str | Path | None = None,
        session_metadata: dict[str, Any] | None = None,
    ) -> "AgentTraceRecorder | None":
        config = load_trace_config(options)
        if not config.enabled:
            return None
        run = RunRecord(
            run_id=str(trace.run_id),
            trace_id=str(trace.trace_id),
            session_id=str(trace.session_id),
            agent_name=str(trace.agent_name),
            model_id=getattr(trace, "model_id", None),
            provider_id=getattr(trace, "provider_id", None),
            workspace=getattr(trace, "workspace", None),
            started_at_ms=int(getattr(trace, "started_at_ms", 0) or _now_ms()),
        )
        return cls(run=run, config=config, base_dir=base_dir, session_metadata=session_metadata)

    @property
    def root_dir(self) -> Path:
        root = Path(self.config.root_dir)
        if not root.is_absolute():
            root = self.base_dir / root
        return root

    @property
    def run_dir(self) -> Path:
        return self.root_dir / self.run.run_id

    @property
    def trace_path(self) -> Path:
        return self.run_dir / "trace.jsonl"

    @property
    def summary_path(self) -> Path:
        return self.run_dir / "summary.json"

    @property
    def process_path(self) -> Path:
        return self.run_dir / "process.md"

    @property
    def artifacts_dir(self) -> Path:
        return self.run_dir / "artifacts"

    def start_run(self, *, attributes: dict[str, Any] | None = None) -> TraceEvent | None:
        return self.record_event("run.started", kind="run", attributes=attributes)

    def finish_run(self, *, status: str = "completed", attributes: dict[str, Any] | None = None) -> TraceEvent | None:
        payload = dict(attributes or {})
        payload.setdefault("status", status)
        return self.record_event("run.finished", kind="run", attributes=payload)

    def fail_run(self, *, error: ErrorRecord | dict[str, Any] | None = None) -> TraceEvent | None:
        attrs = _to_dict(error) if error is not None else {}
        return self.record_event("run.failed", kind="run", status="error", attributes=attrs)

    def start_step(self, step: StepRecord | dict[str, Any]) -> TraceEvent | None:
        return self.record_event("step.started", kind="step", attributes=_to_dict(step))

    def finish_step(self, step: StepRecord | dict[str, Any]) -> TraceEvent | None:
        attrs = _to_dict(step)
        status = "error" if attrs.get("status") == "error" else "ok"
        return self.record_event("step.finished", kind="step", status=status, attributes=attrs)

    def record_model_call(self, record: ModelCallRecord | dict[str, Any]) -> TraceEvent | None:
        attrs = _to_dict(record)
        status = "error" if attrs.get("status") == "error" else "ok"
        return self.record_event("model.call", kind="model", status=status, attributes=attrs, duration_ms=attrs.get("latency_ms"))

    def record_tool_call(self, record: ToolCallRecord | dict[str, Any]) -> TraceEvent | None:
        attrs = _to_dict(record)
        status = "error" if attrs.get("status") == "error" else "ok"
        return self.record_event("tool.call", kind="tool", status=status, attributes=attrs, duration_ms=attrs.get("latency_ms"))

    def record_artifact(self, record: ArtifactRecord | dict[str, Any]) -> TraceEvent | None:
        return self.record_event("artifact.created", kind="artifact", attributes=_to_dict(record))

    def record_observation(self, observation_event: Any) -> TraceEvent | None:
        payload = _to_dict(observation_event)
        return self.record_event(
            str(payload.get("name") or payload.get("event") or "event"),
            event_id=payload.get("event_id"),
            kind=str(payload.get("kind") or "event"),
            status="error" if payload.get("status") == "error" else "ok",
            span_id=payload.get("span_id"),
            parent_span_id=payload.get("parent_span_id"),
            duration_ms=_optional_int(payload.get("duration_ms")),
            timestamp_ms=_optional_int(payload.get("timestamp_ms")),
            attributes=dict(payload.get("attributes") or {}),
        )

    def record_event(
        self,
        event: str,
        *,
        event_id: str | None = None,
        kind: str = "event",
        status: str = "ok",
        span_id: str | None = None,
        parent_span_id: str | None = None,
        duration_ms: int | None = None,
        timestamp_ms: int | None = None,
        attributes: dict[str, Any] | None = None,
    ) -> TraceEvent | None:
        if not self.config.enabled:
            return None
        self._seq += 1
        trace_event = TraceEvent(
            seq=self._seq,
            event=event,
            event_id=event_id,
            timestamp_ms=timestamp_ms or _now_ms(),
            run_id=self.run.run_id,
            trace_id=self.run.trace_id,
            session_id=self.run.session_id,
            kind=kind,
            status="error" if status == "error" else "ok",
            span_id=span_id,
            parent_span_id=parent_span_id,
            duration_ms=duration_ms,
            attributes=sanitize_trace_value(attributes or {}),
        )
        event_dict = trace_event.to_dict()
        self._append_jsonl(event_dict)
        if self.config.keep_events:
            self._events.append(event_dict)
            self._events = self._events[-max(1, self.config.max_events):]
        self._update_summary(event_dict)
        if self.config.write_summary:
            self._write_summary()
        if event in {"run.finished", "run.failed"}:
            status_label = "failed" if trace_event.status == "error" or event == "run.failed" else "completed"
            self._write_process_note(f"Run {status_label} after {self._summary.get('event_count', 0)} trace events.")
        return trace_event

    def summary(self) -> dict[str, Any]:
        return dict(self._summary)

    def _bind_metadata(self) -> None:
        if self.session_metadata is None:
            return
        self.session_metadata[TRACE_METADATA_KEY] = {
            "run_id": self.run.run_id,
            "trace_id": self.run.trace_id,
            "run_dir": str(self.run_dir),
            "trace_path": str(self.trace_path),
            "summary_path": str(self.summary_path),
            "process_path": str(self.process_path),
        }

    def _empty_summary(self) -> dict[str, Any]:
        return {
            **self.run.to_dict(),
            "status": "running",
            "started_at_ms": self.run.started_at_ms,
            "ended_at_ms": None,
            "duration_ms": None,
            "event_count": 0,
            "step_count": 0,
            "model_call_count": 0,
            "tool_call_count": 0,
            "mcp_call_count": 0,
            "skill_call_count": 0,
            "local_tool_call_count": 0,
            "artifact_count": 0,
            "error_count": 0,
            "total_latency_ms": 0,
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "total_reasoning_tokens": 0,
            "total_cache_read_tokens": 0,
            "total_cache_write_tokens": 0,
            "total_cost": 0.0,
            "errors": [],
            "paths": {
                "run_dir": str(self.run_dir),
                "trace": str(self.trace_path),
                "summary": str(self.summary_path),
                "process": str(self.process_path),
                "artifacts": str(self.artifacts_dir),
            },
        }

    def _update_summary(self, event: dict[str, Any]) -> None:
        attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
        self._summary["event_count"] = int(self._summary.get("event_count") or 0) + 1
        duration_ms = _optional_int(event.get("duration_ms"))
        if duration_ms is not None:
            self._summary["total_latency_ms"] = int(self._summary.get("total_latency_ms") or 0) + duration_ms

        name = str(event.get("event") or "")
        kind = str(event.get("kind") or "")
        if event.get("status") == "error":
            self._summary["error_count"] = int(self._summary.get("error_count") or 0) + 1
            self._append_error_summary(event)
        if name == "run.finished":
            self._summary["status"] = str(attrs.get("status") or "completed")
            self._summary["ended_at_ms"] = event.get("timestamp_ms")
        elif name == "run.failed":
            self._summary["status"] = "failed"
            self._summary["ended_at_ms"] = event.get("timestamp_ms")
        elif name == "step.finished":
            self._summary["step_count"] = int(self._summary.get("step_count") or 0) + 1
        elif name == "model.call.finished":
            self._summary["model_call_count"] = int(self._summary.get("model_call_count") or 0) + 1
            self._summary["total_input_tokens"] = int(self._summary.get("total_input_tokens") or 0) + int(attrs.get("input_tokens") or 0)
            self._summary["total_output_tokens"] = int(self._summary.get("total_output_tokens") or 0) + int(attrs.get("output_tokens") or 0)
            self._summary["total_reasoning_tokens"] = int(self._summary.get("total_reasoning_tokens") or 0) + int(attrs.get("reasoning_tokens") or 0)
            self._summary["total_cache_read_tokens"] = int(self._summary.get("total_cache_read_tokens") or 0) + int(attrs.get("cache_read_tokens") or 0)
            self._summary["total_cache_write_tokens"] = int(self._summary.get("total_cache_write_tokens") or 0) + int(attrs.get("cache_write_tokens") or 0)
            self._summary["total_cost"] = float(self._summary.get("total_cost") or 0.0) + float(attrs.get("cost") or 0.0)
        elif name == "tool.call.finished":
            self._summary["tool_call_count"] = int(self._summary.get("tool_call_count") or 0) + 1
            source = _tool_source(attrs)
            if source == "mcp":
                self._summary["mcp_call_count"] = int(self._summary.get("mcp_call_count") or 0) + 1
            elif source == "skill":
                self._summary["skill_call_count"] = int(self._summary.get("skill_call_count") or 0) + 1
            elif source in {"local_tool", "local"}:
                self._summary["local_tool_call_count"] = int(self._summary.get("local_tool_call_count") or 0) + 1
            if attrs.get("output_path"):
                self._summary["artifact_count"] = int(self._summary.get("artifact_count") or 0) + 1
        elif name == "artifact.created" or kind == "artifact":
            self._summary["artifact_count"] = int(self._summary.get("artifact_count") or 0) + 1

        ended_at = _optional_int(self._summary.get("ended_at_ms")) or _now_ms()
        started_at = _optional_int(self._summary.get("started_at_ms")) or ended_at
        self._summary["duration_ms"] = max(0, ended_at - started_at)

    def _append_error_summary(self, event: dict[str, Any]) -> None:
        errors = list(self._summary.get("errors") or [])
        attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
        error_payload = attrs.get("error") if isinstance(attrs.get("error"), dict) else attrs
        errors.append(
            {
                "event": event.get("event"),
                "timestamp_ms": event.get("timestamp_ms"),
                "error_kind": error_payload.get("error_kind") or error_payload.get("type") if isinstance(error_payload, dict) else None,
                "message": error_payload.get("message") if isinstance(error_payload, dict) else None,
            }
        )
        self._summary["errors"] = errors[-10:]

    def _append_jsonl(self, event: dict[str, Any]) -> None:
        self.trace_path.parent.mkdir(parents=True, exist_ok=True)
        with self.trace_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(event, ensure_ascii=False, sort_keys=True) + "\n")

    def _write_summary(self) -> None:
        self.summary_path.parent.mkdir(parents=True, exist_ok=True)
        self.summary_path.write_text(json.dumps(self._summary, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    def _write_process_note(self, message: str) -> None:
        self.process_path.parent.mkdir(parents=True, exist_ok=True)
        existing = self.process_path.read_text(encoding="utf-8") if self.process_path.exists() else "# Trace Process\n\n"
        timestamp = time.strftime("%Y-%m-%d %H:%M:%S %z")
        self.process_path.write_text(existing.rstrip() + f"\n- {timestamp}: {message}\n", encoding="utf-8")


def load_trace_config(options: dict[str, Any] | None) -> TraceConfig:
    raw_options = options or {}
    raw = raw_options.get("trace", {})
    if raw is None:
        raw = {}
    if not isinstance(raw, dict):
        raw = {}
    return TraceConfig(
        enabled=_bool_option(raw.get("enabled", True)),
        root_dir=str(raw.get("root_dir") or raw.get("jsonl_dir") or DEFAULT_TRACE_ROOT),
        keep_events=_bool_option(raw.get("keep_events", True)),
        max_events=_positive_int(raw.get("max_events"), 2000),
        write_summary=_bool_option(raw.get("write_summary", True)),
    )


def load_trace_events(path: str | Path) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        item = json.loads(line)
        if isinstance(item, dict):
            events.append(item)
    return events


def load_trace_summary(path: str | Path) -> dict[str, Any]:
    return json.loads(Path(path).read_text(encoding="utf-8"))


def render_trace_summary(summary: dict[str, Any]) -> str:
    lines = [
        f"Run: {summary.get('run_id')}",
        f"Status: {summary.get('status')}",
        f"Duration: {int(summary.get('duration_ms') or 0)}ms",
        f"Events: {summary.get('event_count', 0)}",
        f"Steps: {summary.get('step_count', 0)}",
        f"Model calls: {summary.get('model_call_count', 0)}",
        f"Tool calls: {summary.get('tool_call_count', 0)}",
        f"MCP calls: {summary.get('mcp_call_count', 0)}",
        f"Skill calls: {summary.get('skill_call_count', 0)}",
        f"Tokens: {summary.get('total_input_tokens', 0)}/{summary.get('total_output_tokens', 0)}",
        f"Reasoning tokens: {summary.get('total_reasoning_tokens', 0)}",
        f"Cache tokens: read={summary.get('total_cache_read_tokens', 0)} write={summary.get('total_cache_write_tokens', 0)}",
        f"Cost: {float(summary.get('total_cost') or 0.0):.6f}",
        f"Errors: {summary.get('error_count', 0)}",
    ]
    return "\n".join(lines) + "\n"


def check_trace_run(run_dir: str | Path) -> dict[str, Any]:
    run_path = Path(run_dir)
    trace_path = run_path / "trace.jsonl"
    summary_path = run_path / "summary.json"
    errors: list[str] = []
    if not trace_path.exists():
        errors.append("missing trace.jsonl")
        events: list[dict[str, Any]] = []
    else:
        events = load_trace_events(trace_path)
    summary: dict[str, Any] = {}
    if not summary_path.exists():
        errors.append("missing summary.json")
    else:
        summary = load_trace_summary(summary_path)

    names = [str(event.get("event") or "") for event in events]
    seqs = [int(event.get("seq") or 0) for event in events]
    if not events:
        errors.append("trace has no events")
    if seqs and seqs != list(range(1, len(seqs) + 1)):
        errors.append("event seq values are not contiguous from 1")
    if "run.started" not in names:
        errors.append("missing run.started")
    if not any(name in names for name in ("run.finished", "run.failed")):
        errors.append("missing terminal run event")
    if "step.started" not in names:
        errors.append("missing step.started")
    if "step.finished" not in names:
        errors.append("missing step.finished")
    if "model.call.started" not in names:
        errors.append("missing model.call.started")
    if "model.call.finished" not in names:
        errors.append("missing model.call.finished")
    run_ids = {event.get("run_id") for event in events if event.get("run_id")}
    if len(run_ids) > 1:
        errors.append("events contain multiple run_id values")
    summary_run_id = summary.get("run_id")
    if summary_run_id and run_ids and summary_run_id not in run_ids:
        errors.append("summary run_id does not match trace events")
    if int(summary.get("event_count") or 0) != len(events) and summary:
        errors.append("summary event_count does not match trace length")

    return {
        "ok": not errors,
        "run_id": summary.get("run_id") or next(iter(run_ids), None),
        "event_count": len(events),
        "errors": errors,
    }


def find_run_dir(run_id: str, *, root: str | Path = DEFAULT_TRACE_ROOT, base_dir: str | Path | None = None) -> Path:
    root_path = Path(root)
    if not root_path.is_absolute():
        root_path = (Path(base_dir) if base_dir is not None else Path.cwd()) / root_path
    direct = root_path / run_id
    if direct.is_dir():
        return direct
    matches = sorted(path for path in root_path.glob(f"**/{run_id}") if path.is_dir()) if root_path.exists() else []
    if matches:
        return matches[0]
    raise FileNotFoundError(f"Trace run not found: {run_id}")


def list_runs(*, root: str | Path = DEFAULT_TRACE_ROOT, base_dir: str | Path | None = None) -> list[dict[str, Any]]:
    root_path = Path(root)
    if not root_path.is_absolute():
        root_path = (Path(base_dir) if base_dir is not None else Path.cwd()) / root_path
    if not root_path.exists():
        return []
    runs: list[dict[str, Any]] = []
    for summary_path in sorted(root_path.glob("**/summary.json")):
        try:
            summary = load_trace_summary(summary_path)
        except Exception:  # noqa: BLE001
            continue
        runs.append(summary)
    return runs


def _to_dict(value: Any) -> dict[str, Any]:
    if value is None:
        return {}
    if isinstance(value, dict):
        return dict(value)
    if is_dataclass(value):
        return asdict(value)
    to_dict = getattr(value, "to_dict", None)
    if callable(to_dict):
        result = to_dict()
        if isinstance(result, dict):
            return result
    return {"value": repr(value)}


def _tool_source(attributes: dict[str, Any]) -> str:
    source = str(attributes.get("tool_source") or attributes.get("source") or "").strip()
    if source:
        return source
    backend = str(attributes.get("backend") or "").strip()
    if backend == "mcp":
        return "mcp"
    group = str(attributes.get("tool_group") or "").strip()
    if group == "skill" or attributes.get("skill_name"):
        return "skill"
    if group:
        return "local_tool"
    return "unknown"


def sanitize_trace_value(value: Any, *, max_chars: int = 4096) -> Any:
    if isinstance(value, dict):
        sanitized: dict[str, Any] = {}
        for key, item in value.items():
            key_text = str(key)
            if _is_sensitive_key(key_text) and key_text not in SAFE_TOKEN_METRIC_KEYS:
                sanitized[key_text] = "[redacted]"
            else:
                sanitized[key_text] = sanitize_trace_value(item, max_chars=max_chars)
        return sanitized
    if isinstance(value, list):
        return [sanitize_trace_value(item, max_chars=max_chars) for item in value]
    if isinstance(value, tuple):
        return [sanitize_trace_value(item, max_chars=max_chars) for item in value]
    if isinstance(value, set):
        return sorted((sanitize_trace_value(item, max_chars=max_chars) for item in value), key=repr)
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, (str, int, float, bool)) or value is None:
        return _truncate_text(value, max_chars) if isinstance(value, str) else value
    return _truncate_text(repr(value), max_chars)


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


def _optional_int(value: Any) -> int | None:
    if value is None:
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _now_ms() -> int:
    return int(time.time() * 1000)


__all__ = [
    "DEFAULT_TRACE_ROOT",
    "TRACE_METADATA_KEY",
    "AgentTraceRecorder",
    "find_run_dir",
    "check_trace_run",
    "list_runs",
    "load_trace_config",
    "load_trace_events",
    "load_trace_summary",
    "render_trace_summary",
]
