from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator
from dataclasses import dataclass, field
from typing import Any

from ..core.id import new_id
from ..core.provider.base import LanguageModel, coerce_usage
from ..core.types import AgentConfig, ChatMessage, FinishReason, StreamEvent, ToolCall, ToolSchema, Usage


@dataclass(frozen=True, slots=True)
class StepInfo:
    finish_reason: FinishReason = "unknown"
    usage: Usage = field(default_factory=Usage)
    tool_calls: list[ToolCall] = field(default_factory=list)


class AgentReplyStream:
    def __init__(self, gen: AsyncIterator[StreamEvent], info_future: "asyncio.Future[StepInfo]") -> None:
        self._gen = gen
        self._info_future = info_future

    def __aiter__(self) -> AsyncIterator[StreamEvent]:
        return self._gen

    async def info(self) -> StepInfo:
        return await self._info_future

    async def aclose(self) -> None:
        close = getattr(self._gen, "aclose", None)
        if close is not None:
            await close()


class AgentAdapter:
    def __init__(self, *, model: LanguageModel, config: AgentConfig) -> None:
        self._model = model
        self._config = config

    def reply_stream(
        self,
        *,
        system: str | None,
        messages: list[ChatMessage],
        tools: list[ToolSchema],
        max_output_tokens: int | None = None,
    ) -> AgentReplyStream:
        loop = asyncio.get_running_loop()
        info_future: "asyncio.Future[StepInfo]" = loop.create_future()
        tool_calls: list[ToolCall] = []
        finish_reason: FinishReason = "unknown"
        usage: Usage = Usage()

        async def _gen() -> AsyncIterator[StreamEvent]:
            nonlocal finish_reason, usage
            text_id = new_id("text")
            started = False
            try:
                async for ev in self._model.stream(
                    system=system,
                    messages=messages,
                    tools=tools,
                    temperature=self._config.temperature,
                    max_output_tokens=max_output_tokens if max_output_tokens is not None else (self._config.model.max_output if self._config.model else None),
                    options=self._config.options,
                ):
                    et = ev.get("type")
                    if et == "text-delta":
                        if not started:
                            started = True
                            yield {"type": "text-start", "id": text_id, "metadata": ev.get("metadata")}  # type: ignore[misc]
                        yield {"type": "text-delta", "id": text_id, "text": str(ev.get("text", ""))}  # type: ignore[misc]
                    elif et == "tool-call":
                        call = ToolCall(
                            name=str(ev["name"]),
                            input=dict(ev.get("input") or {}),
                            call_id=str(ev.get("call_id") or ev.get("tool_call_id") or new_id("toolcall")),
                        )
                        tool_calls.append(call)
                        yield {"type": "tool-call", "name": call.name, "input": call.input, "call_id": call.call_id}  # type: ignore[misc]
                    elif et == "finish":
                        finish_reason = str(ev.get("finish_reason") or "unknown")  # type: ignore[assignment]
                        usage = coerce_usage(ev.get("usage"))
                    else:
                        continue
                if started:
                    yield {"type": "text-end", "id": text_id}  # type: ignore[misc]
            finally:
                if not info_future.done():
                    info_future.set_result(StepInfo(finish_reason=finish_reason, usage=usage, tool_calls=tool_calls))

        return AgentReplyStream(_gen(), info_future)
