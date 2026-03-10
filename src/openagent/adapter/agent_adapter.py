from __future__ import annotations

"""
AgentAdapter：把 Provider 的 LanguageModel 转换为 OpenAgent 可消费的“流事件”。

为什么需要 Adapter？
- Provider 的 SDK/接口千差万别（OpenAI/Anthropic/阿里等）
- 我们希望 AgentLoop 只依赖统一的 StreamEvent（见 `core/types.py`）

本模块做的事：
- 调用 model.stream(...) 获取 provider 事件
- 统一封装为：
  - text-start / text-delta / text-end（便于前端/控制台流式展示）
  - tool-call（用于触发 Toolkit 执行）
  - finish（被转换为 StepInfo：usage + finish_reason + tool_calls）

注意：
- 这里不负责执行工具，只负责“解析/转发模型输出”
- 工具执行由 AgentLoop 完成（并将 tool-result 写回 session messages）
"""

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
                    max_output_tokens=self._config.model.max_output if self._config.model else None,
                    options=self._config.options,
                ):
                    et = ev.get("type")
                    if et == "text-delta":
                        if not started:
                            started = True
                            # 第一次输出文本片段时，补一个 text-start 事件
                            yield {"type": "text-start", "id": text_id, "metadata": ev.get("metadata")}  # type: ignore[misc]
                        yield {"type": "text-delta", "id": text_id, "text": str(ev.get("text", ""))}  # type: ignore[misc]
                    elif et == "tool-call":
                        # 模型请求调用工具：这里只“记录 + 透传”，不执行
                        call = ToolCall(
                            name=str(ev["name"]),
                            input=dict(ev.get("input") or {}),
                            call_id=str(ev.get("call_id") or ev.get("tool_call_id") or new_id("toolcall")),
                        )
                        tool_calls.append(call)
                        yield {"type": "tool-call", "name": call.name, "input": call.input, "call_id": call.call_id}  # type: ignore[misc]
                    elif et == "finish":
                        # finish 事件通常在最后出现，包含 usage/finish_reason
                        finish_reason = str(ev.get("finish_reason") or "unknown")  # type: ignore[assignment]
                        usage = coerce_usage(ev.get("usage"))
                    else:
                        continue
            finally:
                if started:
                    # 文本结束：补 text-end，便于上层做收尾处理（例如 trim、落盘等）
                    yield {"type": "text-end", "id": text_id}  # type: ignore[misc]
                if not info_future.done():
                    # StepInfo 会被 AgentLoop 用来决定是否执行 tool-calls、是否继续循环等
                    info_future.set_result(StepInfo(finish_reason=finish_reason, usage=usage, tool_calls=tool_calls))

        return AgentReplyStream(_gen(), info_future)
