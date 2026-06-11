from __future__ import annotations

import hashlib
import os
from typing import Any, Protocol

from .schema import RunRecord, TraceConfig

LANGSMITH_OTEL_TRACE_ENDPOINT = "https://api.smith.langchain.com/otel/v1/traces"
CONTENT_KEY_MARKERS = (
    "arguments_preview",
    "completion",
    "content",
    "input_preview",
    "message",
    "output",
    "prompt",
    "result_summary",
    "text",
)
SAFE_CONTENT_KEYS = {
    "input_chars",
    "input_tokens",
    "output_bytes",
    "output_lines",
    "output_path",
    "output_tokens",
    "output_truncated",
    "total_chars",
    "truncated",
}


class TraceExporter(Protocol):
    name: str

    def record_event(self, event: dict[str, Any]) -> None:
        """Export one sanitized trace event."""

    def close(self) -> None:
        """Flush and release exporter resources."""


def build_trace_exporters(
    *,
    run: RunRecord,
    config: TraceConfig,
    diagnostics: list[dict[str, Any]] | None = None,
) -> list[TraceExporter]:
    exporters: list[TraceExporter] = []
    raw = config.exporters if isinstance(config.exporters, dict) else {}
    langsmith = raw.get("langsmith")
    if isinstance(langsmith, dict) and _bool_option(langsmith.get("enabled", False)):
        try:
            exporters.append(LangSmithOtelExporter.from_config(run=run, options=langsmith))
        except Exception as error:  # noqa: BLE001
            diagnostic = {
                "exporter": "langsmith",
                "status": "error",
                "error_kind": type(error).__name__,
                "message": str(error),
            }
            if diagnostics is not None:
                diagnostics.append(diagnostic)
            if _bool_option(langsmith.get("strict", False)):
                raise
    langfuse = raw.get("langfuse")
    if isinstance(langfuse, dict) and _bool_option(langfuse.get("enabled", False)):
        try:
            exporters.append(LangfuseTraceExporter.from_config(run=run, options=langfuse))
        except Exception as error:  # noqa: BLE001
            diagnostic = {
                "exporter": "langfuse",
                "status": "error",
                "error_kind": type(error).__name__,
                "message": str(error),
            }
            if diagnostics is not None:
                diagnostics.append(diagnostic)
            if _bool_option(langfuse.get("strict", False)):
                raise
    return exporters


class LangfuseTraceExporter:
    """Minimal Langfuse exporter for OpenAgent trace events.

    Local trace files remain the source of truth. This exporter mirrors run,
    step, model, and tool spans into Langfuse with content export disabled by
    default.
    """

    name = "langfuse"

    def __init__(
        self,
        *,
        run: RunRecord,
        client: Any,
        langfuse_trace_id: str,
        include_content: bool = False,
        include_workspace: bool = False,
        environment: str = "local",
        tags: list[str] | None = None,
        scores_enabled: bool = True,
    ) -> None:
        self.run = run
        self.client = client
        self.langfuse_trace_id = langfuse_trace_id
        self.include_content = include_content
        self.include_workspace = include_workspace
        self.environment = environment
        self.tags = list(tags or ["openagent"])
        self.scores_enabled = scores_enabled
        self._closed = False
        self._active_step_key: str | None = None
        self._observations: dict[str, Any] = {}
        self._observation_ids: dict[str, str] = {}

    @classmethod
    def from_config(cls, *, run: RunRecord, options: dict[str, Any]) -> "LangfuseTraceExporter":
        client = cls._load_client(options)
        trace_id = _langfuse_trace_id(client, run.trace_id)
        return cls(
            run=run,
            client=client,
            langfuse_trace_id=trace_id,
            include_content=_bool_option(options.get("include_content", False)),
            include_workspace=_bool_option(options.get("include_workspace", False)),
            environment=str(options.get("environment") or os.getenv("LANGFUSE_ENVIRONMENT") or "local"),
            tags=_string_list(options.get("tags"), default=["openagent"]),
            scores_enabled=_bool_option(options.get("scores_enabled", True)),
        )

    @staticmethod
    def _load_client(options: dict[str, Any]) -> Any:
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

    def record_event(self, event: dict[str, Any]) -> None:
        if self._closed:
            return
        name = str(event.get("event") or "")
        if name in {"run.started", "step.started", "model.call.started", "tool.call.started"}:
            self._start_observation(event)
            return
        if name in {
            "run.finished",
            "run.failed",
            "step.finished",
            "step.failed",
            "model.call.finished",
            "model.call.failed",
            "tool.call.finished",
            "tool.call.failed",
        }:
            self._finish_observation(event)

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
            "scores_enabled": self.scores_enabled,
            "include_content": self.include_content,
            "include_workspace": self.include_workspace,
            "environment": self.environment,
            "tags": list(self.tags),
        }

    def _start_observation(self, event: dict[str, Any]) -> None:
        key = self._observation_key(event)
        if not key or key in self._observations:
            return
        trace_context = {"trace_id": self.langfuse_trace_id}
        parent_id = self._parent_observation_id(event, key)
        if parent_id:
            trace_context["parent_span_id"] = parent_id
        observation = self._start_client_observation(
            name=self._observation_name(event),
            as_type=self._observation_type(event),
            trace_context=trace_context,
        )
        self._update_observation(observation, event, terminal=False)
        self._observations[key] = observation
        observation_id = str(getattr(observation, "id", "") or getattr(observation, "observation_id", "") or "")
        if observation_id:
            self._observation_ids[key] = observation_id
        if str(event.get("event")) == "step.started":
            self._active_step_key = key

    def _finish_observation(self, event: dict[str, Any]) -> None:
        key = self._observation_key(event)
        if not key:
            return
        observation = self._observations.pop(key, None)
        if observation is None:
            self._start_missing_observation(event, key)
            observation = self._observations.pop(key, None)
        if observation is None:
            return
        self._update_observation(observation, event, terminal=True)
        _safe_end(observation)
        if self._active_step_key == key:
            self._active_step_key = None

    def _start_missing_observation(self, event: dict[str, Any], key: str) -> None:
        synthetic = dict(event)
        synthetic["event"] = _started_event_name(str(event.get("event") or ""))
        self._start_observation(synthetic)
        if key not in self._observations:
            self._observations[key] = self._start_client_observation(
                name=self._observation_name(synthetic),
                as_type=self._observation_type(synthetic),
                trace_context={"trace_id": self.langfuse_trace_id},
            )

    def _start_client_observation(self, *, name: str, as_type: str, trace_context: dict[str, str]) -> Any:
        method = getattr(self.client, "start_observation", None)
        if not callable(method):
            raise RuntimeError("Langfuse client does not support start_observation().")
        try:
            return method(name=name, as_type=as_type, trace_context=trace_context)
        except TypeError:
            return method(name=name, as_type=as_type)

    def _update_observation(self, observation: Any, event: dict[str, Any], *, terminal: bool) -> None:
        attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
        metadata = self._metadata_payload(event)
        payload: dict[str, Any] = {"metadata": metadata}
        observation_type = self._observation_type(event)
        if observation_type == "generation":
            model = attrs.get("model") or self.run.model_id
            if model:
                payload["model"] = str(model)
            usage = _usage_details(attrs)
            if usage:
                payload["usage_details"] = usage
            cost = _float_value(attrs.get("cost"))
            if cost is not None:
                payload["cost_details"] = {"total": cost}
        if terminal:
            payload["level"] = "ERROR" if event.get("status") == "error" else "DEFAULT"
            if event.get("status") == "error":
                payload["status_message"] = _error_message(attrs)
        if self.include_content:
            content = _content_payload(attrs, observation_type=observation_type)
            payload.update(content)
        _safe_update(observation, payload)

    def _metadata_payload(self, event: dict[str, Any]) -> dict[str, Any]:
        attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
        payload: dict[str, Any] = {
            "openagent_event": str(event.get("event") or ""),
            "openagent_seq": int(event.get("seq") or 0),
            "openagent_status": str(event.get("status") or "ok"),
            "openagent_kind": str(event.get("kind") or "event"),
            "openagent_run_id": self.run.run_id,
            "openagent_trace_id": self.run.trace_id,
            "openagent_session_id": self.run.session_id,
            "openagent_agent_name": self.run.agent_name,
            "langfuse_environment": self.environment,
            "langfuse_tags": list(self.tags),
        }
        if self.run.model_id:
            payload["openagent_model_id"] = self.run.model_id
        if self.run.provider_id:
            payload["openagent_provider_id"] = self.run.provider_id
        if self.include_workspace and self.run.workspace:
            payload["openagent_workspace"] = self.run.workspace
        duration_ms = event.get("duration_ms")
        if isinstance(duration_ms, (int, float)):
            payload["openagent_duration_ms"] = duration_ms
        for key, value in attrs.items():
            if not self._should_export_attribute(str(key)):
                continue
            payload[f"attr_{_normalize_attr_key(str(key))}"] = value
        return payload

    def _should_export_attribute(self, key: str) -> bool:
        if self.include_content:
            return True
        lowered = key.lower()
        if key in SAFE_CONTENT_KEYS:
            return True
        return not any(marker in lowered for marker in CONTENT_KEY_MARKERS)

    def _parent_observation_id(self, event: dict[str, Any], key: str) -> str | None:
        parent_span_id = str(event.get("parent_span_id") or "").strip()
        if parent_span_id:
            return self._observation_ids.get(f"span:{parent_span_id}")
        if key.startswith("step:"):
            return self._observation_ids.get("run")
        if key.startswith("span:"):
            return self._observation_ids.get(self._active_step_key or "") or self._observation_ids.get("run")
        return None

    def _observation_key(self, event: dict[str, Any]) -> str | None:
        name = str(event.get("event") or "")
        attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
        if name.startswith("run."):
            return "run"
        if name.startswith("step."):
            step_index = attrs.get("step_index")
            return f"step:{step_index}" if step_index is not None else f"step:{event.get('seq')}"
        span_id = str(event.get("span_id") or "").strip()
        if span_id:
            return f"span:{span_id}"
        if name.startswith(("model.call", "tool.call")):
            return f"{name}:{event.get('seq')}"
        return None

    def _observation_name(self, event: dict[str, Any]) -> str:
        name = str(event.get("event") or "")
        attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
        if name.startswith("run."):
            return f"openagent.run {self.run.agent_name}"
        if name.startswith("step."):
            return f"openagent.step {attrs.get('step_index', '')}".rstrip()
        if name.startswith("model.call"):
            return f"model.call {attrs.get('model') or self.run.model_id or 'model'}"
        if name.startswith("tool.call"):
            return f"tool.call {attrs.get('tool_name') or 'tool'}"
        return name

    def _observation_type(self, event: dict[str, Any]) -> str:
        name = str(event.get("event") or "")
        kind = str(event.get("kind") or "")
        if name.startswith("run.") or kind == "run":
            return "agent"
        if name.startswith("model.call") or kind == "model":
            return "generation"
        if name.startswith("tool.call") or kind == "tool":
            return "tool"
        return "span"


class LangSmithOtelExporter:
    """Minimal OpenTelemetry exporter for LangSmith.

    This adapter sends OpenAgent run/step/model/tool spans only. Content-like
    attributes are excluded by default and require explicit opt-in.
    """

    name = "langsmith"

    def __init__(
        self,
        *,
        run: RunRecord,
        endpoint: str,
        headers: dict[str, str],
        service_name: str,
        include_content: bool = False,
        include_workspace: bool = False,
    ) -> None:
        self.run = run
        self.include_content = include_content
        self.include_workspace = include_workspace
        self._closed = False
        self._active_step_key: str | None = None
        self._spans: dict[str, Any] = {}
        self._provider, self._tracer, self._otel_trace, self._otel_status = self._setup_otel(
            endpoint=endpoint,
            headers=headers,
            service_name=service_name,
        )

    @classmethod
    def from_config(cls, *, run: RunRecord, options: dict[str, Any]) -> "LangSmithOtelExporter":
        endpoint = str(options.get("endpoint") or os.getenv("LANGSMITH_OTEL_ENDPOINT") or LANGSMITH_OTEL_TRACE_ENDPOINT)
        project = str(options.get("project") or os.getenv("LANGSMITH_PROJECT") or "").strip()
        api_key_env = str(options.get("api_key_env") or "LANGSMITH_API_KEY")
        api_key = str(options.get("api_key") or os.getenv(api_key_env) or "").strip()
        headers = _headers(options.get("headers"))
        if api_key:
            headers.setdefault("x-api-key", api_key)
        if project:
            headers.setdefault("Langsmith-Project", project)
        api_key_required = _bool_option(options.get("api_key_required", True))
        if api_key_required and not any(key.lower() == "x-api-key" for key in headers):
            raise ValueError(f"LangSmith exporter is enabled but no API key was provided via trace config or {api_key_env}.")
        return cls(
            run=run,
            endpoint=endpoint,
            headers=headers,
            service_name=str(options.get("service_name") or os.getenv("OTEL_SERVICE_NAME") or "openagent"),
            include_content=_bool_option(options.get("include_content", False)),
            include_workspace=_bool_option(options.get("include_workspace", False)),
        )

    def record_event(self, event: dict[str, Any]) -> None:
        if self._closed:
            return
        name = str(event.get("event") or "")
        if name in {"run.started", "step.started", "model.call.started", "tool.call.started"}:
            self._start_span(event)
            return
        if name in {
            "run.finished",
            "run.failed",
            "step.finished",
            "step.failed",
            "model.call.finished",
            "model.call.failed",
            "tool.call.finished",
            "tool.call.failed",
        }:
            self._finish_span(event)

    def close(self) -> None:
        if self._closed:
            return
        for key in sorted(self._spans, reverse=True):
            span = self._spans.pop(key)
            span.end()
        self._provider.shutdown()
        self._closed = True

    def _setup_otel(self, *, endpoint: str, headers: dict[str, str], service_name: str) -> tuple[Any, Any, Any, Any]:
        try:
            from opentelemetry import trace as otel_trace
            from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
            from opentelemetry.sdk.resources import Resource
            from opentelemetry.sdk.trace import TracerProvider
            from opentelemetry.sdk.trace.export import BatchSpanProcessor
            from opentelemetry.trace import Status, StatusCode
        except ImportError as error:
            raise RuntimeError(
                "LangSmith exporter requires optional dependencies: "
                "pip install 'openagent-core[langsmith]'"
            ) from error

        resource = Resource.create(
            {
                "service.name": service_name,
                "openagent.agent.name": self.run.agent_name,
                "openagent.run_id": self.run.run_id,
                "openagent.trace_id": self.run.trace_id,
            }
        )
        provider = TracerProvider(resource=resource)
        exporter = OTLPSpanExporter(endpoint=endpoint, headers=headers)
        provider.add_span_processor(BatchSpanProcessor(exporter))
        return provider, provider.get_tracer("openagent.core.trace"), otel_trace, (Status, StatusCode)

    def _start_span(self, event: dict[str, Any]) -> None:
        key = self._span_key(event)
        if not key or key in self._spans:
            return
        context = self._parent_context(event, key)
        span = self._tracer.start_span(
            self._span_name(event),
            context=context,
            start_time=_timestamp_ns(event),
        )
        self._set_common_attributes(span, event)
        self._set_event_attributes(span, event)
        self._spans[key] = span
        if str(event.get("event")) == "step.started":
            self._active_step_key = key

    def _finish_span(self, event: dict[str, Any]) -> None:
        key = self._span_key(event)
        if not key:
            return
        span = self._spans.pop(key, None)
        if span is None:
            self._start_missing_span(event, key)
            span = self._spans.pop(key, None)
        if span is None:
            return
        self._set_common_attributes(span, event)
        self._set_event_attributes(span, event)
        if event.get("status") == "error" or str(event.get("event") or "").endswith(".failed"):
            status_cls, status_code_cls = self._otel_status
            attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
            description = str(attrs.get("error_kind") or attrs.get("type") or "error")
            span.set_status(status_cls(status_code_cls.ERROR, description))
        span.end(end_time=_timestamp_ns(event))
        if self._active_step_key == key:
            self._active_step_key = None

    def _start_missing_span(self, event: dict[str, Any], key: str) -> None:
        started_name = _started_event_name(str(event.get("event") or ""))
        synthetic = dict(event)
        synthetic["event"] = started_name
        self._start_span(synthetic)
        if key not in self._spans:
            self._spans[key] = self._tracer.start_span(self._span_name(synthetic), start_time=_timestamp_ns(event))

    def _parent_context(self, event: dict[str, Any], key: str) -> Any:
        parent_key: str | None = None
        parent_span_id = str(event.get("parent_span_id") or "").strip()
        if parent_span_id:
            parent_key = f"span:{parent_span_id}"
        elif key.startswith("step:"):
            parent_key = "run"
        elif key.startswith("span:"):
            parent_key = self._active_step_key or "run"
        elif key != "run":
            parent_key = "run"
        parent_span = self._spans.get(parent_key or "")
        return self._otel_trace.set_span_in_context(parent_span) if parent_span is not None else None

    def _span_key(self, event: dict[str, Any]) -> str | None:
        name = str(event.get("event") or "")
        attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
        if name.startswith("run."):
            return "run"
        if name.startswith("step."):
            step_index = attrs.get("step_index")
            return f"step:{step_index}" if step_index is not None else f"step:{event.get('seq')}"
        span_id = str(event.get("span_id") or "").strip()
        if span_id:
            return f"span:{span_id}"
        if name.startswith(("model.call", "tool.call")):
            return f"{name}:{event.get('seq')}"
        return None

    def _span_name(self, event: dict[str, Any]) -> str:
        name = str(event.get("event") or "")
        attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
        if name.startswith("run."):
            return f"openagent.run {self.run.agent_name}"
        if name.startswith("step."):
            return f"openagent.step {attrs.get('step_index', '')}".rstrip()
        if name.startswith("model.call"):
            model = attrs.get("model") or self.run.model_id or "model"
            return f"model.call {model}"
        if name.startswith("tool.call"):
            tool = attrs.get("tool_name") or "tool"
            return f"tool.call {tool}"
        return name

    def _set_common_attributes(self, span: Any, event: dict[str, Any]) -> None:
        span.set_attribute("langsmith.span.kind", _langsmith_span_kind(event))
        span.set_attribute("openagent.event", str(event.get("event") or ""))
        span.set_attribute("openagent.seq", int(event.get("seq") or 0))
        span.set_attribute("openagent.status", str(event.get("status") or "ok"))
        span.set_attribute("openagent.kind", str(event.get("kind") or "event"))
        span.set_attribute("openagent.run_id", self.run.run_id)
        span.set_attribute("openagent.trace_id", self.run.trace_id)
        span.set_attribute("openagent.session_id", self.run.session_id)
        span.set_attribute("openagent.agent.name", self.run.agent_name)
        if self.run.model_id:
            span.set_attribute("openagent.model.id", self.run.model_id)
        if self.run.provider_id:
            span.set_attribute("openagent.provider.id", self.run.provider_id)
        if self.include_workspace and self.run.workspace:
            span.set_attribute("openagent.workspace", self.run.workspace)
        if event.get("span_id"):
            span.set_attribute("openagent.span_id", str(event["span_id"]))
        if event.get("parent_span_id"):
            span.set_attribute("openagent.parent_span_id", str(event["parent_span_id"]))
        duration_ms = event.get("duration_ms")
        if isinstance(duration_ms, (int, float)):
            span.set_attribute("openagent.duration_ms", duration_ms)

    def _set_event_attributes(self, span: Any, event: dict[str, Any]) -> None:
        attrs = event.get("attributes") if isinstance(event.get("attributes"), dict) else {}
        if str(event.get("event") or "").startswith("model.call"):
            _set_if_present(span, "gen_ai.system", attrs.get("provider") or self.run.provider_id)
            _set_if_present(span, "gen_ai.request.model", attrs.get("model") or self.run.model_id)
            input_tokens = _int_value(attrs.get("input_tokens"))
            output_tokens = _int_value(attrs.get("output_tokens"))
            if input_tokens is not None:
                span.set_attribute("gen_ai.usage.prompt_tokens", input_tokens)
            if output_tokens is not None:
                span.set_attribute("gen_ai.usage.completion_tokens", output_tokens)
            if input_tokens is not None or output_tokens is not None:
                span.set_attribute("gen_ai.usage.total_tokens", int(input_tokens or 0) + int(output_tokens or 0))
        if str(event.get("event") or "").startswith("tool.call"):
            _set_if_present(span, "openagent.tool.name", attrs.get("tool_name"))
            _set_if_present(span, "openagent.tool.source", attrs.get("tool_source") or attrs.get("source"))
            _set_if_present(span, "openagent.mcp.server", attrs.get("mcp_server"))
            _set_if_present(span, "openagent.skill.name", attrs.get("skill_name"))
        for key, value in attrs.items():
            if not self._should_export_attribute(str(key)):
                continue
            safe_value = _otel_attribute_value(value)
            if safe_value is not None:
                span.set_attribute(f"openagent.attr.{_normalize_attr_key(str(key))}", safe_value)

    def _should_export_attribute(self, key: str) -> bool:
        if self.include_content:
            return True
        lowered = key.lower()
        if key in SAFE_CONTENT_KEYS:
            return True
        return not any(marker in lowered for marker in CONTENT_KEY_MARKERS)


def load_langfuse_client(options: dict[str, Any] | None) -> Any:
    return LangfuseTraceExporter._load_client(dict(options or {}))


def _langsmith_span_kind(event: dict[str, Any]) -> str:
    kind = str(event.get("kind") or "")
    name = str(event.get("event") or "")
    if kind == "model" or name.startswith("model.call"):
        return "llm"
    if kind == "tool" or name.startswith("tool.call"):
        return "tool"
    return "chain"


def _timestamp_ns(event: dict[str, Any]) -> int | None:
    value = event.get("timestamp_ms")
    try:
        return int(value) * 1_000_000
    except (TypeError, ValueError):
        return None


def _started_event_name(name: str) -> str:
    if name.endswith(".finished"):
        return name[: -len(".finished")] + ".started"
    if name.endswith(".failed"):
        return name[: -len(".failed")] + ".started"
    return name


def _headers(value: Any) -> dict[str, str]:
    if not isinstance(value, dict):
        return {}
    return {str(key): str(item) for key, item in value.items() if item is not None}


def _set_if_present(span: Any, key: str, value: Any) -> None:
    safe = _otel_attribute_value(value)
    if safe is not None:
        span.set_attribute(key, safe)


def _otel_attribute_value(value: Any) -> Any:
    if isinstance(value, (str, bool, int, float)):
        return value
    if value is None:
        return None
    if isinstance(value, list):
        items = [_otel_attribute_value(item) for item in value]
        filtered = [item for item in items if isinstance(item, (str, bool, int, float))]
        return filtered if len(filtered) == len(items) else None
    return str(value)


def _normalize_attr_key(key: str) -> str:
    return "".join(char if char.isalnum() else "_" for char in key).strip("_") or "value"


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


def _usage_details(attrs: dict[str, Any]) -> dict[str, int]:
    usage: dict[str, int] = {}
    input_tokens = _int_value(attrs.get("input_tokens"))
    output_tokens = _int_value(attrs.get("output_tokens"))
    reasoning_tokens = _int_value(attrs.get("reasoning_tokens"))
    cache_read_tokens = _int_value(attrs.get("cache_read_tokens"))
    cache_write_tokens = _int_value(attrs.get("cache_write_tokens"))
    if input_tokens is not None:
        usage["input_tokens"] = input_tokens
    if output_tokens is not None:
        usage["output_tokens"] = output_tokens
    if reasoning_tokens is not None:
        usage["reasoning_tokens"] = reasoning_tokens
    if cache_read_tokens is not None:
        usage["cache_read_tokens"] = cache_read_tokens
    if cache_write_tokens is not None:
        usage["cache_write_tokens"] = cache_write_tokens
    if input_tokens is not None or output_tokens is not None:
        usage["total_tokens"] = int(input_tokens or 0) + int(output_tokens or 0)
    return usage


def _content_payload(attrs: dict[str, Any], *, observation_type: str) -> dict[str, Any]:
    payload: dict[str, Any] = {}
    if observation_type == "generation":
        input_value = _first_present(attrs, "prompt", "input_preview", "messages")
        output_value = _first_present(attrs, "completion", "output", "text")
    elif observation_type == "tool":
        input_value = _first_present(attrs, "arguments_preview", "input", "input_preview")
        output_value = _first_present(attrs, "result_summary", "output", "output_preview")
    else:
        input_value = _first_present(attrs, "input", "input_preview")
        output_value = _first_present(attrs, "output", "result_summary")
    if input_value is not None:
        payload["input"] = input_value
    if output_value is not None:
        payload["output"] = output_value
    return payload


def _first_present(attrs: dict[str, Any], *keys: str) -> Any:
    for key in keys:
        if key in attrs and attrs[key] is not None:
            return attrs[key]
    return None


def _error_message(attrs: dict[str, Any]) -> str | None:
    if attrs.get("message"):
        return str(attrs["message"])
    error = attrs.get("error")
    if isinstance(error, dict):
        return str(error.get("message") or error.get("error_kind") or error.get("type") or "error")
    if attrs.get("error_kind"):
        return str(attrs["error_kind"])
    return None


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
    "LANGSMITH_OTEL_TRACE_ENDPOINT",
    "LangfuseTraceExporter",
    "LangSmithOtelExporter",
    "TraceExporter",
    "build_trace_exporters",
    "load_langfuse_client",
]
