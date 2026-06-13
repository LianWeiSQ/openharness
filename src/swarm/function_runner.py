from __future__ import annotations

"""Function-mode runner adapter.

This is the smallest useful runner type: a plain Python callable becomes an
agent endpoint. It gives the kernel an executable P0 without depending on
OpenAgent, subprocesses, or remote HTTP services.
"""

import inspect
from collections.abc import AsyncIterator, Awaitable, Callable, Mapping
from typing import Any

from .config import SwarmConfig
from .protocol import AgentDescriptor, AgentEvent, AgentResult, AgentSpec, RunContext
from .registry import RunnerRegistry
from .results import normalize_result_payload

FunctionResult = AgentResult | str | dict[str, Any]
FunctionHandler = Callable[[AgentSpec, RunContext], FunctionResult | Awaitable[FunctionResult]]


class InMemoryRunHandle:
    def __init__(self, *, events: list[AgentEvent], result: AgentResult) -> None:
        self._events = events
        self._result = result
        self._cancelled = False

    async def events(self) -> AsyncIterator[AgentEvent]:
        for event in self._events:
            yield event

    async def result(self) -> AgentResult:
        if self._cancelled:
            return AgentResult(status="cancelled", summary="Function runner was cancelled.")
        return self._result

    async def cancel(self) -> None:
        self._cancelled = True


class FunctionRunner:
    def __init__(self, *, descriptor: AgentDescriptor, handler: FunctionHandler) -> None:
        self._descriptor = descriptor
        self._handler = handler

    @property
    def descriptor(self) -> AgentDescriptor:
        return self._descriptor

    async def start(self, spec: AgentSpec, ctx: RunContext) -> InMemoryRunHandle:
        spec.validate()
        started = AgentEvent(
            type="runner.started",
            run_id=ctx.run_id,
            runner_id=self.descriptor.id,
            message=f"Started {self.descriptor.id}",
            metadata={"role": spec.role, "kind": self.descriptor.kind},
        )
        try:
            raw = self._handler(spec, ctx)
            if inspect.isawaitable(raw):
                raw = await raw
            result = normalize_function_result(raw)
        except Exception as error:  # noqa: BLE001
            result = AgentResult(
                status="failed",
                summary=str(error),
                metadata={"error_kind": "function_runner_error", "runner_id": self.descriptor.id},
            )

        finished = AgentEvent(
            type="runner.finished",
            run_id=ctx.run_id,
            runner_id=self.descriptor.id,
            message=result.summary,
            metadata={"status": result.status, "confidence": result.confidence},
        )
        return InMemoryRunHandle(events=[started, finished], result=result)


def normalize_function_result(value: FunctionResult) -> AgentResult:
    return normalize_result_payload(value)


def build_function_registry(
    config: SwarmConfig,
    functions: Mapping[str, FunctionHandler],
) -> RunnerRegistry:
    registry = RunnerRegistry()
    for runner_config in config.runners:
        if runner_config.kind != "function":
            continue
        handler_key = runner_config.handler or runner_config.id
        handler = functions.get(handler_key) or functions.get(runner_config.id)
        if handler is None:
            raise KeyError(f'function handler "{handler_key}" for runner "{runner_config.id}" is not registered')
        registry.register(
            FunctionRunner(
                descriptor=runner_config.to_descriptor(),
                handler=handler,
            )
        )
    return registry


__all__ = ["FunctionHandler", "FunctionRunner", "InMemoryRunHandle", "build_function_registry"]
