from __future__ import annotations

"""HTTP runner for remote swarm agents."""

import asyncio
import contextlib
import json
import socket
import urllib.error
import urllib.request
from collections.abc import AsyncIterator
from dataclasses import dataclass, field
from typing import Any

from .config import SwarmConfig
from .payload import payload_for_runner
from .protocol import AgentDescriptor, AgentEvent, AgentResult, AgentSpec, RunContext
from .registry import RunnerRegistry
from .results import normalize_result_payload


@dataclass(frozen=True, slots=True)
class HttpRequestConfig:
    url: str
    method: str = "POST"
    headers: dict[str, str] = field(default_factory=dict)
    timeout_seconds: float | None = None


@dataclass(frozen=True, slots=True)
class _HttpResponse:
    status: int
    body: str
    headers: dict[str, str] = field(default_factory=dict)


class HttpRunHandle:
    def __init__(self, *, events: list[AgentEvent], result: AgentResult) -> None:
        self._events = events
        self._result = result
        self._cancelled = False

    async def events(self) -> AsyncIterator[AgentEvent]:
        for event in self._events:
            yield event

    async def result(self) -> AgentResult:
        if self._cancelled:
            return AgentResult(status="cancelled", summary="HTTP runner was cancelled.")
        return self._result

    async def cancel(self) -> None:
        self._cancelled = True


class HttpRunner:
    def __init__(self, *, descriptor: AgentDescriptor, request: HttpRequestConfig) -> None:
        if not request.url.strip():
            raise ValueError("http runner metadata.url is required")
        self._descriptor = descriptor
        self.request = request

    @property
    def descriptor(self) -> AgentDescriptor:
        return self._descriptor

    async def start(self, spec: AgentSpec, ctx: RunContext) -> HttpRunHandle:
        spec.validate()
        started = AgentEvent(
            type="runner.started",
            run_id=ctx.run_id,
            runner_id=self.descriptor.id,
            message=f"Started {self.descriptor.id}",
            metadata={"role": spec.role, "kind": self.descriptor.kind, "transport": "http"},
        )
        result = await self._run_request(spec=spec, ctx=ctx)
        finished = AgentEvent(
            type="runner.finished",
            run_id=ctx.run_id,
            runner_id=self.descriptor.id,
            message=result.summary,
            metadata={"status": result.status, "confidence": result.confidence, "transport": "http"},
        )
        return HttpRunHandle(events=[started, finished], result=result)

    async def _run_request(self, *, spec: AgentSpec, ctx: RunContext) -> AgentResult:
        timeout = spec.limits.timeout_seconds or self.request.timeout_seconds or 30.0
        payload = payload_for_runner(
            spec=spec,
            ctx=ctx,
            descriptor=self.descriptor,
            runner_metadata=_public_runner_metadata(self.descriptor.metadata),
        )
        try:
            response = await asyncio.to_thread(
                _perform_request,
                request=self.request,
                payload=payload,
                timeout=timeout,
            )
        except urllib.error.HTTPError as error:
            body = _read_error_body(error)
            return AgentResult(
                status="failed",
                summary=body or str(error),
                metadata={
                    "error_kind": "http_status_error",
                    "runner_id": self.descriptor.id,
                    "http_status": error.code,
                },
            )
        except (TimeoutError, socket.timeout) as error:
            return AgentResult(
                status="failed",
                summary=f"HTTP runner timed out after {timeout} seconds.",
                metadata={"error_kind": "http_timeout", "runner_id": self.descriptor.id, "error": str(error)},
            )
        except urllib.error.URLError as error:
            reason = getattr(error, "reason", error)
            return AgentResult(
                status="failed",
                summary=str(reason),
                metadata={"error_kind": "http_request_error", "runner_id": self.descriptor.id},
            )
        except Exception as error:  # noqa: BLE001
            return AgentResult(
                status="failed",
                summary=str(error),
                metadata={"error_kind": "http_request_error", "runner_id": self.descriptor.id},
            )

        if not response.body.strip():
            return AgentResult(
                status="completed",
                summary="HTTP runner completed without response body.",
                metadata={"runner_id": self.descriptor.id, "http_status": response.status},
            )
        try:
            decoded = json.loads(response.body)
        except json.JSONDecodeError:
            return AgentResult(
                status="completed",
                summary=response.body.strip(),
                metadata={
                    "runner_id": self.descriptor.id,
                    "http_status": response.status,
                    "response_format": "text",
                },
            )
        result = normalize_result_payload(decoded if isinstance(decoded, dict) else str(decoded))
        return AgentResult(
            status=result.status,
            summary=result.summary,
            evidence=list(result.evidence),
            open_questions=list(result.open_questions),
            confidence=result.confidence,
            artifacts=list(result.artifacts),
            usage=result.usage,
            metadata={
                **dict(result.metadata or {}),
                "runner_id": self.descriptor.id,
                "http_status": response.status,
                "response_format": "json",
            },
        )


def build_http_registry(config: SwarmConfig) -> RunnerRegistry:
    registry = RunnerRegistry()
    for runner_config in config.runners:
        if runner_config.kind != "http":
            continue
        registry.register(
            HttpRunner(
                descriptor=runner_config.to_descriptor(),
                request=_request_from_metadata(runner_config.metadata),
            )
        )
    return registry


def _request_from_metadata(metadata: dict[str, Any]) -> HttpRequestConfig:
    raw_url = str(metadata.get("url") or "").strip()
    if not raw_url:
        raise ValueError("http runner metadata.url is required")
    raw_headers = metadata.get("headers") or {}
    if not isinstance(raw_headers, dict):
        raise ValueError("http runner headers must be a mapping")
    timeout = metadata.get("timeout_seconds")
    return HttpRequestConfig(
        url=raw_url,
        method=str(metadata.get("method") or "POST").upper(),
        headers={str(key): str(value) for key, value in raw_headers.items()},
        timeout_seconds=float(timeout) if timeout is not None else None,
    )


def _perform_request(*, request: HttpRequestConfig, payload: dict[str, Any], timeout: float) -> _HttpResponse:
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    headers = {"Content-Type": "application/json", "Accept": "application/json", **request.headers}
    req = urllib.request.Request(
        request.url,
        data=body,
        headers=headers,
        method=request.method,
    )
    with urllib.request.urlopen(req, timeout=timeout) as response:  # noqa: S310 - URL is user-configured runner endpoint.
        response_body = response.read().decode("utf-8", errors="replace")
        return _HttpResponse(
            status=int(response.status),
            body=response_body,
            headers={str(key): str(value) for key, value in response.headers.items()},
        )


def _read_error_body(error: urllib.error.HTTPError) -> str:
    try:
        return error.read().decode("utf-8", errors="replace").strip()
    except Exception:  # noqa: BLE001
        return ""
    finally:
        with contextlib.suppress(Exception):
            error.close()


def _public_runner_metadata(metadata: dict[str, Any]) -> dict[str, Any]:
    blocked = {"url", "headers", "authorization", "api_key", "token", "secret", "password"}
    return {str(key): value for key, value in metadata.items() if str(key).lower() not in blocked}


__all__ = ["HttpRequestConfig", "HttpRunHandle", "HttpRunner", "build_http_registry"]
