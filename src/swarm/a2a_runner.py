from __future__ import annotations

"""A2A HTTP+JSON runner for standard remote agents."""

import asyncio
import contextlib
import json
import socket
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import AsyncIterator
from dataclasses import dataclass, field
from typing import Any
from uuid import uuid4

from .config import SwarmConfig
from .protocol import AgentDescriptor, AgentEvent, AgentResult, AgentSpec, RunContext
from .registry import RunnerRegistry


@dataclass(frozen=True, slots=True)
class A2ARequestConfig:
    url: str
    headers: dict[str, str] = field(default_factory=dict)
    timeout_seconds: float | None = None
    version: str = "1.0"
    accepted_output_modes: list[str] = field(default_factory=lambda: ["text/plain"])
    streaming: bool = False


@dataclass(frozen=True, slots=True)
class _A2AResponse:
    status: int
    body: str
    headers: dict[str, str] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class _A2AStreamResponse:
    status: int
    events: list[dict[str, Any]]
    headers: dict[str, str] = field(default_factory=dict)


class A2ARunHandle:
    def __init__(self, *, events: list[AgentEvent], result: AgentResult) -> None:
        self._events = events
        self._result = result
        self._cancelled = False

    async def events(self) -> AsyncIterator[AgentEvent]:
        for event in self._events:
            yield event

    async def result(self) -> AgentResult:
        if self._cancelled:
            return AgentResult(status="cancelled", summary="A2A runner was cancelled.")
        return self._result

    async def cancel(self) -> None:
        self._cancelled = True


class A2ARunner:
    def __init__(self, *, descriptor: AgentDescriptor, request: A2ARequestConfig) -> None:
        if not request.url.strip():
            raise ValueError("a2a runner metadata.url is required")
        self._descriptor = descriptor
        self.request = request

    @property
    def descriptor(self) -> AgentDescriptor:
        return self._descriptor

    async def start(self, spec: AgentSpec, ctx: RunContext) -> A2ARunHandle:
        spec.validate()
        started = AgentEvent(
            type="runner.started",
            run_id=ctx.run_id,
            runner_id=self.descriptor.id,
            message=f"Started {self.descriptor.id}",
            metadata={"role": spec.role, "kind": self.descriptor.kind, "transport": "a2a-http-json"},
        )
        result, stream_events = await self._send_message(spec=spec, ctx=ctx)
        finished = AgentEvent(
            type="runner.finished",
            run_id=ctx.run_id,
            runner_id=self.descriptor.id,
            message=result.summary,
            metadata={"status": result.status, "confidence": result.confidence, "transport": "a2a-http-json"},
        )
        return A2ARunHandle(events=[started, *stream_events, finished], result=result)

    async def _send_message(self, *, spec: AgentSpec, ctx: RunContext) -> tuple[AgentResult, list[AgentEvent]]:
        timeout = spec.limits.timeout_seconds or self.request.timeout_seconds or 30.0
        payload = _payload_for_a2a(spec=spec, ctx=ctx, runner_id=self.descriptor.id, request=self.request)
        try:
            if self.request.streaming:
                stream_response = await asyncio.to_thread(
                    _perform_stream_request,
                    request=self.request,
                    payload=payload,
                    timeout=timeout,
                )
                return _result_from_stream_response(response=stream_response, runner_id=self.descriptor.id, run_id=ctx.run_id)
            response = await asyncio.to_thread(
                _perform_request,
                request=self.request,
                payload=payload,
                timeout=timeout,
            )
        except urllib.error.HTTPError as error:
            body = _read_error_body(error)
            return (
                AgentResult(
                    status="failed",
                    summary=body or str(error),
                    metadata={"error_kind": "a2a_http_status_error", "runner_id": self.descriptor.id, "http_status": error.code},
                ),
                [],
            )
        except (TimeoutError, socket.timeout) as error:
            return (
                AgentResult(
                    status="failed",
                    summary=f"A2A runner timed out after {timeout} seconds.",
                    metadata={"error_kind": "a2a_timeout", "runner_id": self.descriptor.id, "error": str(error)},
                ),
                [],
            )
        except urllib.error.URLError as error:
            reason = getattr(error, "reason", error)
            return (
                AgentResult(
                    status="failed",
                    summary=str(reason),
                    metadata={"error_kind": "a2a_request_error", "runner_id": self.descriptor.id},
                ),
                [],
            )
        except Exception as error:  # noqa: BLE001
            return (
                AgentResult(
                    status="failed",
                    summary=str(error),
                    metadata={"error_kind": "a2a_request_error", "runner_id": self.descriptor.id},
                ),
                [],
            )
        return _result_from_response(response=response, runner_id=self.descriptor.id), []


def build_a2a_registry(config: SwarmConfig) -> RunnerRegistry:
    registry = RunnerRegistry()
    for runner_config in config.runners:
        if runner_config.kind != "a2a":
            continue
        registry.register(
            A2ARunner(
                descriptor=runner_config.to_descriptor(),
                request=_request_from_metadata(runner_config.metadata),
            )
        )
    return registry


def _request_from_metadata(metadata: dict[str, Any]) -> A2ARequestConfig:
    raw_url = str(metadata.get("url") or "").strip()
    if not raw_url:
        raise ValueError("a2a runner metadata.url is required")
    raw_headers = metadata.get("headers") or {}
    if not isinstance(raw_headers, dict):
        raise ValueError("a2a runner headers must be a mapping")
    timeout = metadata.get("timeout_seconds")
    accepted = metadata.get("accepted_output_modes") or ["text/plain"]
    if isinstance(accepted, str):
        accepted_modes = [accepted]
    elif isinstance(accepted, list):
        accepted_modes = [str(item) for item in accepted]
    else:
        raise ValueError("a2a accepted_output_modes must be a string or list")
    streaming = bool(metadata.get("streaming") or metadata.get("stream"))
    return A2ARequestConfig(
        url=_message_stream_url(raw_url) if streaming else _message_send_url(raw_url),
        headers={str(key): str(value) for key, value in raw_headers.items()},
        timeout_seconds=float(timeout) if timeout is not None else None,
        version=str(metadata.get("version") or "1.0"),
        accepted_output_modes=accepted_modes,
        streaming=streaming,
    )


def _payload_for_a2a(*, spec: AgentSpec, ctx: RunContext, runner_id: str, request: A2ARequestConfig) -> dict[str, Any]:
    text = "\n".join(
        [
            f"Role: {spec.role}",
            f"Objective: {spec.objective}",
            f"Context: {spec.context}",
            f"Boundaries: {spec.boundaries}",
            f"Output schema: {json.dumps(spec.output_schema, ensure_ascii=False, sort_keys=True)}",
            f"Inputs: {json.dumps(spec.inputs, ensure_ascii=False, sort_keys=True)}",
        ]
    )
    return {
        "message": {
            "role": "ROLE_USER",
            "parts": [{"text": text}],
            "messageId": f"{ctx.run_id}:{runner_id}:{uuid4().hex}",
            "contextId": ctx.run_id,
        },
        "configuration": {
            "acceptedOutputModes": list(request.accepted_output_modes),
            "metadata": {
                "swarm_run_id": ctx.run_id,
                "swarm_runner_id": runner_id,
                "swarm_parent_span_id": ctx.parent_span_id,
            },
        },
    }


def _perform_request(*, request: A2ARequestConfig, payload: dict[str, Any], timeout: float) -> _A2AResponse:
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    headers = {
        "Content-Type": "application/a2a+json",
        "Accept": "application/a2a+json",
        "A2A-Version": request.version,
        **request.headers,
    }
    req = urllib.request.Request(request.url, data=body, headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=timeout) as response:  # noqa: S310 - URL is user-configured A2A endpoint.
        response_body = response.read().decode("utf-8", errors="replace")
        return _A2AResponse(
            status=int(response.status),
            body=response_body,
            headers={str(key): str(value) for key, value in response.headers.items()},
        )


def _perform_stream_request(*, request: A2ARequestConfig, payload: dict[str, Any], timeout: float) -> _A2AStreamResponse:
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    headers = {
        "Content-Type": "application/a2a+json",
        "Accept": "text/event-stream",
        "A2A-Version": request.version,
        **request.headers,
    }
    req = urllib.request.Request(request.url, data=body, headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=timeout) as response:  # noqa: S310 - URL is user-configured A2A endpoint.
        events = list(_iter_sse_events(response))
        return _A2AStreamResponse(
            status=int(response.status),
            events=events,
            headers={str(key): str(value) for key, value in response.headers.items()},
        )


def _result_from_response(*, response: _A2AResponse, runner_id: str) -> AgentResult:
    if not response.body.strip():
        return AgentResult(
            status="completed",
            summary="A2A runner completed without response body.",
            metadata={"runner_id": runner_id, "http_status": response.status, "response_format": "empty"},
        )
    try:
        decoded = json.loads(response.body)
    except json.JSONDecodeError:
        return AgentResult(
            status="completed",
            summary=response.body.strip(),
            metadata={"runner_id": runner_id, "http_status": response.status, "response_format": "text"},
        )
    if not isinstance(decoded, dict):
        return AgentResult(
            status="completed",
            summary=str(decoded),
            metadata={"runner_id": runner_id, "http_status": response.status, "response_format": "json"},
        )
    if isinstance(decoded.get("error"), dict):
        error = decoded["error"]
        return AgentResult(
            status="failed",
            summary=str(error.get("message") or error),
            metadata={
                "error_kind": "a2a_error_response",
                "runner_id": runner_id,
                "http_status": response.status,
                "a2a_error_code": error.get("code"),
            },
        )

    task = decoded.get("task") if isinstance(decoded.get("task"), dict) else None
    message = decoded.get("message") if isinstance(decoded.get("message"), dict) else None
    metadata: dict[str, Any] = {"runner_id": runner_id, "http_status": response.status, "response_format": "a2a-json"}
    if task:
        metadata["a2a_task_id"] = task.get("id")
        status = task.get("status") if isinstance(task.get("status"), dict) else {}
        state = str(status.get("state") or "")
        if state:
            metadata["a2a_task_state"] = state
        summary = _summary_from_task(task) or _summary_from_message(status.get("message")) or state or "A2A task response."
        return AgentResult(
            status=_status_from_task_state(state),
            summary=summary,
            evidence=_artifact_names(task),
            confidence=0.0,
            metadata=metadata,
        )
    if message:
        return AgentResult(
            status="completed",
            summary=_summary_from_message(message) or "A2A message response.",
            metadata=metadata,
        )
    return AgentResult(status="completed", summary=str(decoded), metadata=metadata)


def _result_from_stream_response(*, response: _A2AStreamResponse, runner_id: str, run_id: str) -> tuple[AgentResult, list[AgentEvent]]:
    stream_events: list[AgentEvent] = []
    summary_chunks: list[str] = []
    evidence: list[str] = []
    latest_state = ""
    task_id = None
    latest_task: dict[str, Any] | None = None
    direct_message: dict[str, Any] | None = None

    for index, raw_event in enumerate(response.events):
        event = _unwrap_stream_event(raw_event)
        kind = _stream_event_kind(event)
        if kind == "task":
            task = event.get("task") if isinstance(event.get("task"), dict) else event
            latest_task = task
            task_id = task.get("id") or task_id
            status = task.get("status") if isinstance(task.get("status"), dict) else {}
            latest_state = str(status.get("state") or latest_state)
            text = _summary_from_task(task)
            if text:
                summary_chunks.append(text)
            evidence.extend(_artifact_names(task))
        elif kind == "message":
            direct_message = event.get("message") if isinstance(event.get("message"), dict) else event
            text = _summary_from_message(direct_message)
            if text:
                summary_chunks.append(text)
        elif kind == "statusUpdate":
            update = event.get("statusUpdate") if isinstance(event.get("statusUpdate"), dict) else event
            task_id = update.get("taskId") or update.get("task_id") or task_id
            status = update.get("status") if isinstance(update.get("status"), dict) else {}
            latest_state = str(status.get("state") or latest_state)
            message_text = _summary_from_message(status.get("message"))
            if message_text:
                summary_chunks.append(message_text)
        elif kind == "artifactUpdate":
            update = event.get("artifactUpdate") if isinstance(event.get("artifactUpdate"), dict) else event
            task_id = update.get("taskId") or update.get("task_id") or task_id
            artifact = update.get("artifact") if isinstance(update.get("artifact"), dict) else {}
            artifact_text = _text_from_parts(artifact.get("parts"))
            if artifact_text:
                summary_chunks.append(artifact_text)
            artifact_name = artifact.get("name") or artifact.get("artifactId") or artifact.get("artifact_id")
            if artifact_name:
                evidence.append(str(artifact_name))
        stream_events.append(
            AgentEvent(
                type=f"a2a.stream.{kind}",
                run_id=run_id,
                runner_id=runner_id,
                message=_stream_event_message(kind=kind, event=event),
                metadata={
                    "transport": "a2a-http-json",
                    "streaming": True,
                    "event_index": index,
                    "event_kind": kind,
                    **_stream_event_metadata(event),
                },
            )
        )

    metadata: dict[str, Any] = {
        "runner_id": runner_id,
        "http_status": response.status,
        "response_format": "a2a-sse",
        "a2a_stream_events": len(response.events),
        "streaming": True,
    }
    if task_id:
        metadata["a2a_task_id"] = task_id
    if latest_state:
        metadata["a2a_task_state"] = latest_state

    if direct_message and not latest_task and not latest_state:
        return (
            AgentResult(
                status="completed",
                summary=_summary_from_message(direct_message) or "A2A stream message response.",
                metadata=metadata,
            ),
            stream_events,
        )

    summary = _dedupe_join(summary_chunks) or (latest_state if latest_state else "A2A stream completed.")
    return (
        AgentResult(
            status=_status_from_task_state(latest_state),
            summary=summary,
            evidence=_dedupe(evidence),
            metadata=metadata,
        ),
        stream_events,
    )


def _status_from_task_state(state: str) -> str:
    normalized = state.upper()
    if normalized.endswith("COMPLETED") or normalized == "COMPLETED":
        return "completed"
    if (
        normalized.endswith("FAILED")
        or normalized.endswith("REJECTED")
        or normalized.endswith("CANCELED")
        or normalized.endswith("CANCELLED")
    ):
        return "failed"
    if normalized.endswith("INPUT_REQUIRED") or normalized.endswith("WORKING") or normalized.endswith("SUBMITTED"):
        return "partial"
    return "partial" if normalized else "completed"


def _summary_from_task(task: dict[str, Any]) -> str:
    chunks: list[str] = []
    for artifact in task.get("artifacts") or []:
        if isinstance(artifact, dict):
            text = _text_from_parts(artifact.get("parts"))
            if text:
                chunks.append(text)
    return "\n".join(chunks).strip()


def _summary_from_message(message: Any) -> str:
    if not isinstance(message, dict):
        return ""
    return _text_from_parts(message.get("parts"))


def _text_from_parts(parts: Any) -> str:
    if not isinstance(parts, list):
        return ""
    chunks: list[str] = []
    for part in parts:
        if isinstance(part, dict) and part.get("text") is not None:
            chunks.append(str(part["text"]))
    return "\n".join(chunks).strip()


def _artifact_names(task: dict[str, Any]) -> list[str]:
    names: list[str] = []
    for artifact in task.get("artifacts") or []:
        if isinstance(artifact, dict):
            value = artifact.get("name") or artifact.get("artifactId") or artifact.get("artifact_id")
            if value:
                names.append(str(value))
    return names


def _iter_sse_events(response: Any):
    data_lines: list[str] = []
    while True:
        raw_line = response.readline()
        if raw_line == b"":
            break
        line = raw_line.decode("utf-8", errors="replace").rstrip("\r\n")
        if line == "":
            yield from _decode_sse_data(data_lines)
            data_lines = []
            continue
        if line.startswith(":"):
            continue
        field, separator, value = line.partition(":")
        if not separator:
            continue
        if value.startswith(" "):
            value = value[1:]
        if field == "data":
            data_lines.append(value)
    yield from _decode_sse_data(data_lines)


def _decode_sse_data(data_lines: list[str]):
    if not data_lines:
        return
    data = "\n".join(data_lines).strip()
    if not data or data == "[DONE]":
        return
    try:
        decoded = json.loads(data)
    except json.JSONDecodeError:
        yield {"text": data}
        return
    if isinstance(decoded, dict):
        yield decoded
    else:
        yield {"data": decoded}


def _unwrap_stream_event(event: dict[str, Any]) -> dict[str, Any]:
    result = event.get("result")
    if isinstance(result, dict):
        return result
    return event


def _stream_event_kind(event: dict[str, Any]) -> str:
    if isinstance(event.get("task"), dict):
        return "task"
    if isinstance(event.get("message"), dict):
        return "message"
    if isinstance(event.get("statusUpdate"), dict) or event.get("kind") == "status-update":
        return "statusUpdate"
    if isinstance(event.get("artifactUpdate"), dict) or event.get("kind") == "artifact-update":
        return "artifactUpdate"
    if event.get("id") and isinstance(event.get("status"), dict):
        return "task"
    return "event"


def _stream_event_message(*, kind: str, event: dict[str, Any]) -> str:
    if kind == "message":
        message = event.get("message") if isinstance(event.get("message"), dict) else event
        return _summary_from_message(message)
    if kind == "statusUpdate":
        update = event.get("statusUpdate") if isinstance(event.get("statusUpdate"), dict) else event
        status = update.get("status") if isinstance(update.get("status"), dict) else {}
        return str(status.get("state") or "status update")
    if kind == "artifactUpdate":
        update = event.get("artifactUpdate") if isinstance(event.get("artifactUpdate"), dict) else event
        artifact = update.get("artifact") if isinstance(update.get("artifact"), dict) else {}
        return str(artifact.get("name") or artifact.get("artifactId") or artifact.get("artifact_id") or "artifact update")
    if kind == "task":
        task = event.get("task") if isinstance(event.get("task"), dict) else event
        status = task.get("status") if isinstance(task.get("status"), dict) else {}
        return str(status.get("state") or task.get("id") or "task update")
    return "stream event"


def _stream_event_metadata(event: dict[str, Any]) -> dict[str, Any]:
    metadata: dict[str, Any] = {}
    task = event.get("task") if isinstance(event.get("task"), dict) else event if event.get("id") else {}
    if isinstance(task, dict) and task.get("id"):
        metadata["a2a_task_id"] = task.get("id")
    if isinstance(task, dict) and isinstance(task.get("status"), dict) and task["status"].get("state"):
        metadata["a2a_task_state"] = task["status"].get("state")
    status_update = event.get("statusUpdate") if isinstance(event.get("statusUpdate"), dict) else None
    if status_update:
        metadata["a2a_task_id"] = status_update.get("taskId") or status_update.get("task_id")
        status = status_update.get("status") if isinstance(status_update.get("status"), dict) else {}
        if status.get("state"):
            metadata["a2a_task_state"] = status.get("state")
    artifact_update = event.get("artifactUpdate") if isinstance(event.get("artifactUpdate"), dict) else None
    if artifact_update:
        metadata["a2a_task_id"] = artifact_update.get("taskId") or artifact_update.get("task_id")
        artifact = artifact_update.get("artifact") if isinstance(artifact_update.get("artifact"), dict) else {}
        artifact_name = artifact.get("name") or artifact.get("artifactId") or artifact.get("artifact_id")
        if artifact_name:
            metadata["a2a_artifact"] = artifact_name
    return {key: value for key, value in metadata.items() if value is not None}


def _dedupe(items: list[str]) -> list[str]:
    seen: set[str] = set()
    selected: list[str] = []
    for item in items:
        if item and item not in seen:
            seen.add(item)
            selected.append(item)
    return selected


def _dedupe_join(items: list[str]) -> str:
    return "\n".join(_dedupe([item.strip() for item in items if item.strip()]))


def _message_send_url(url: str) -> str:
    parsed = urllib.parse.urlparse(url)
    path = parsed.path or "/"
    if path.rstrip("/").endswith("/message:send"):
        return url
    base_path = path.rstrip("/")
    next_path = f"{base_path}/message:send" if base_path else "/message:send"
    return urllib.parse.urlunparse(parsed._replace(path=next_path))


def _message_stream_url(url: str) -> str:
    parsed = urllib.parse.urlparse(url)
    path = parsed.path or "/"
    if path.rstrip("/").endswith("/message:stream"):
        return url
    if path.rstrip("/").endswith("/message:send"):
        next_path = path.rstrip("/")[: -len("message:send")] + "message:stream"
        return urllib.parse.urlunparse(parsed._replace(path=next_path))
    base_path = path.rstrip("/")
    next_path = f"{base_path}/message:stream" if base_path else "/message:stream"
    return urllib.parse.urlunparse(parsed._replace(path=next_path))


def _read_error_body(error: urllib.error.HTTPError) -> str:
    try:
        return error.read().decode("utf-8", errors="replace").strip()
    except Exception:  # noqa: BLE001
        return ""
    finally:
        with contextlib.suppress(Exception):
            error.close()


__all__ = ["A2ARequestConfig", "A2ARunHandle", "A2ARunner", "build_a2a_registry"]
