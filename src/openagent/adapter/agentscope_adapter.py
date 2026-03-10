from __future__ import annotations

"""
AgentScopeAgentAdapter：在不改动 AgentLoop 的前提下接入 AgentScope。

关键点（避免双执行）：
- AgentLoop 的工具执行逻辑依赖 StepInfo.tool_calls
- 本 adapter 会把 AgentScope 内部的工具调用过程“直接翻译成 StreamEvent 并输出”
- 因此 StepInfo.tool_calls 始终为空，AgentLoop 不会再二次执行工具

执行归属：
- 工具执行发生在 AgentScope 链路中（ReActAgent 调用工具）
- 但实际工具实现仍复用 OpenAgent 的 ToolkitAdapter（含 Permission middleware）
"""

import asyncio
import inspect
import os
from collections.abc import AsyncIterator, Awaitable, Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

from ..core.permission.manager import PermissionAskRequiredError, PermissionDeniedError, PermissionManager
from ..core.permission.ruleset import PermissionRuleset
from ..core.tool.toolkit import ToolkitAdapter
from ..core.types import AgentConfig, ChatMessage, StreamEvent, Usage
from .agent_adapter import AgentReplyStream, StepInfo
from .memory_adapter import MemoryAdapter
from ._agentscope_compat import make_dashscope_model, require_react_agent


class EventSink(Protocol):
    """用于从“后台执行线程”安全地把事件推回 asyncio 事件循环。"""

    def text(self, delta: str) -> None: ...

    def tool_call(self, *, name: str, input: dict[str, Any], call_id: str) -> None: ...

    def tool_result(self, *, call_id: str, output: str, error: str | None) -> None: ...


@dataclass(slots=True)
class AgentScopeBackendResult:
    text: str
    usage: Usage = Usage()


class AgentScopeBackend(Protocol):
    """
    底层执行器协议（便于测试注入 fake backend，不依赖真实 agentscope）。
    """

    async def run(
        self,
        *,
        system: str | None,
        messages: list[ChatMessage],
        sink: EventSink,
    ) -> AgentScopeBackendResult: ...


class DefaultAgentScopeBackend:
    """
    默认 backend：使用 AgentScope ReActAgent + DashScope/Qwen。

    注意：
    - agentscope API 可能会随版本变化，本实现使用 feature detection + 多种调用方式兜底
    - 若你的 agentscope 分支接口不同，请在 `_agentscope_compat.py` 里补候选路径/适配
    """

    def __init__(
        self,
        *,
        config: AgentConfig,
        session_root: Path,
        permission_manager: PermissionManager,
        toolkit: ToolkitAdapter,
        memory: MemoryAdapter,
    ) -> None:
        self._config = config
        self._session_root = session_root
        self._permission_manager = permission_manager
        self._toolkit = toolkit
        self._memory = memory
        self._agent: Any | None = None
        # 当前 run 的事件 sink（每次 run 会更新，用于把工具事件转发给上层）
        self._sink: EventSink | None = None

    def _ensure_agent(self) -> Any:
        """
        延迟创建 ReActAgent（每个 OpenAgent Session 保持一个实例，保留其内部记忆/状态）。
        """

        if self._agent is not None:
            return self._agent

        api_key = os.getenv("DASHSCOPE_API_KEY") or ""
        if not api_key:
            raise RuntimeError("未检测到环境变量 DASHSCOPE_API_KEY，请先设置后再运行。")

        model_id = os.getenv("DASHSCOPE_MODEL") or (self._config.model.id if self._config.model else "qwen-plus")
        base_url = os.getenv("DASHSCOPE_BASE_URL")

        # 1) 解析并创建 AgentScope 模型 wrapper
        model = make_dashscope_model(api_key=api_key, model=model_id, base_url=base_url)

        # 2) 解析 ReActAgent class
        ReActAgent = require_react_agent()

        # 4) 尝试用不同参数名实例化 ReActAgent（兼容不同版本）
        init = None
        try:
            init = ReActAgent(model=model, tools=self._build_tool_proxies())
        except TypeError:
            try:
                init = ReActAgent(model=model, toolkit=self._build_tool_proxies())
            except TypeError:
                try:
                    init = ReActAgent(llm=model, tools=self._build_tool_proxies())
                except TypeError:
                    init = ReActAgent(model=model)

        self._agent = init
        return self._agent

    def _build_tool_proxies(self) -> dict[str, Callable[..., Awaitable[str]]]:
        """
        构建工具代理函数：
        - AgentScope 调用这些函数
        - 代理函数内部调用 OpenAgent ToolkitAdapter.execute(...)
        - 事件通过 sink 发回上层（tool-call/tool-result）
        """

        async def _call_tool(name: str, params: dict[str, Any]) -> str:
            sink = self._sink
            if sink is None:
                raise RuntimeError("AgentScope tool sink 未初始化（内部错误）")
            call_id = f"as_{name}_{id(params)}"
            sink.tool_call(name=name, input=params, call_id=call_id)
            try:
                result = await self._toolkit.execute(
                    name=name,
                    input=params,
                    call_id=call_id,
                    context={"session_root": str(self._session_root), "memory": self._memory},
                )
            except (PermissionDeniedError, PermissionAskRequiredError) as e:
                sink.tool_result(call_id=call_id, output="", error=str(e))
                raise
            except Exception as e:  # noqa: BLE001
                sink.tool_result(call_id=call_id, output="", error=str(e))
                raise

            sink.tool_result(call_id=call_id, output=result.output, error=result.error)
            if result.error:
                raise RuntimeError(result.error)
            return result.output

        # v1：覆盖 OpenAgent 已实现的工具集合（可按需扩展）
        tool_names = [
            "read",
            "write",
            "edit",
            "glob",
            "grep",
            "ls",
            "bash",
            "code_search",
            "memory_read",
            "memory_write",
            "web_fetch",
            "web_search",
        ]

        proxies: dict[str, Callable[..., Awaitable[str]]] = {}
        for tn in tool_names:
            async def _proxy(*args: Any, tn=tn, **kwargs: Any) -> str:  # type: ignore[misc]
                # 兼容不同 agentscope 调用约定：
                # - 可能传入一个 dict
                # - 也可能传入关键字参数（**kwargs）
                params: dict[str, Any]
                if len(args) == 1 and isinstance(args[0], dict) and not kwargs:
                    params = dict(args[0])
                else:
                    params = dict(kwargs)
                return await _call_tool(tn, params)

            proxies[tn] = _proxy
        return proxies

    async def run(self, *, system: str | None, messages: list[ChatMessage], sink: EventSink) -> AgentScopeBackendResult:
        # 每次 run 都重新应用权限规则集（由 AgentLoop 负责设置 ruleset，但这里兜底一层）
        self._permission_manager.set_ruleset(PermissionRuleset[self._config.permission])
        # 设置本次运行的 sink（供工具代理函数使用）
        self._sink = sink

        agent = self._ensure_agent()
        user_text = ""
        for m in reversed(messages):
            if m.role == "user":
                user_text = m.content
                break
        if not user_text:
            raise RuntimeError("找不到 user message")

        # 尝试调用 AgentScope agent（兼容多种调用方式：__call__/run/reply）
        result = None
        if callable(agent):
            result = agent(user_text)
        elif hasattr(agent, "run"):
            result = agent.run(user_text)
        elif hasattr(agent, "reply"):
            result = agent.reply(user_text)
        else:
            raise RuntimeError("无法调用 AgentScope ReActAgent：未找到 __call__/run/reply")

        if inspect.isawaitable(result):
            result = await result

        # 结果可能是 string / dict / message 对象：尽量提取 text
        text = ""
        if isinstance(result, str):
            text = result
        elif isinstance(result, dict):
            text = str(result.get("content") or result.get("text") or result)
        else:
            text = str(getattr(result, "content", "") or getattr(result, "text", "") or result)

        # 默认 backend 不做真正 token 计费统计；若 agentscope 提供可在此补齐
        sink.text(text)
        return AgentScopeBackendResult(text=text, usage=Usage())


class _AsyncQueueSink:
    """
    线程安全的事件 sink：支持从后台线程投递事件到 asyncio.Queue。
    """

    def __init__(self, *, loop: asyncio.AbstractEventLoop, q: "asyncio.Queue[StreamEvent]") -> None:
        self._loop = loop
        self._q = q
        # 保存 run_coroutine_threadsafe 返回的 Future，用于在结束前 flush，保证事件不丢
        self._pending: list["asyncio.Future[Any]"] = []

    def _put(self, ev: StreamEvent) -> None:
        # 如果 sink 被“同一个事件循环线程”调用，直接 put_nowait 以保证事件顺序
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            running = None
        if running is self._loop:
            self._q.put_nowait(ev)
            return

        # 否则（后台线程/其他 loop），用线程安全方式投递，并记录 Future 便于 flush
        fut = asyncio.run_coroutine_threadsafe(self._q.put(ev), self._loop)
        self._pending.append(asyncio.wrap_future(fut, loop=self._loop))

    async def flush_async(self) -> None:
        """
        等待所有跨线程投递完成，避免 `_done` 先入队导致前面的事件丢失。
        """

        pending = list(self._pending)
        self._pending.clear()
        if not pending:
            return
        # 等待所有 put 协程完成
        await asyncio.gather(*pending, return_exceptions=True)

    def text(self, delta: str) -> None:
        self._put({"type": "text-delta", "id": "agentscope_text", "text": delta})  # type: ignore[misc]

    def tool_call(self, *, name: str, input: dict[str, Any], call_id: str) -> None:
        self._put({"type": "tool-call", "name": name, "input": input, "call_id": call_id})  # type: ignore[misc]

    def tool_result(self, *, call_id: str, output: str, error: str | None) -> None:
        self._put({"type": "tool-result", "call_id": call_id, "output": output, "error": error, "metadata": None})  # type: ignore[misc]


class AgentScopeAgentAdapter:
    """
    对外形态与 AgentAdapter 一致：reply_stream() -> AgentReplyStream。
    """

    def __init__(
        self,
        *,
        config: AgentConfig,
        session_root: Path,
        permission_manager: PermissionManager,
        toolkit: ToolkitAdapter,
        memory: MemoryAdapter,
        backend: AgentScopeBackend | None = None,
    ) -> None:
        self._config = config
        self._session_root = session_root
        self._permission_manager = permission_manager
        self._toolkit = toolkit
        self._memory = memory
        self._backend = backend or DefaultAgentScopeBackend(
            config=config,
            session_root=session_root,
            permission_manager=permission_manager,
            toolkit=toolkit,
            memory=memory,
        )

    def reply_stream(
        self,
        *,
        system: str | None,
        messages: list[ChatMessage],
        tools: list[Any],  # tools 参数保留以匹配 AgentLoop 调用，但此 adapter 可忽略
    ) -> AgentReplyStream:
        loop = asyncio.get_running_loop()
        q: "asyncio.Queue[StreamEvent]" = asyncio.Queue()
        info_future: "asyncio.Future[StepInfo]" = loop.create_future()

        sink = _AsyncQueueSink(loop=loop, q=q)

        async def _run_backend() -> None:
            try:
                res = await self._backend.run(system=system, messages=messages, sink=sink)
                # 确保 backend 内投递的 tool/text 事件全部入队后再结束
                await sink.flush_async()
                if not info_future.done():
                    # 关键：tool_calls 置空，避免 AgentLoop 再执行工具
                    info_future.set_result(StepInfo(finish_reason="stop", usage=res.usage, tool_calls=[]))
            except Exception as e:  # noqa: BLE001
                await q.put({"type": "error", "error": str(e)})  # type: ignore[misc]
                if not info_future.done():
                    info_future.set_result(StepInfo(finish_reason="error", usage=Usage(), tool_calls=[]))
            finally:
                await q.put({"type": "_done"})  # type: ignore[misc]

        async def _gen() -> AsyncIterator[StreamEvent]:
            # 后台任务启动
            task = asyncio.create_task(_run_backend())
            started = False
            text_id = "agentscope_text"
            try:
                while True:
                    ev = await q.get()
                    if ev.get("type") == "_done":
                        break
                    if ev.get("type") == "text-delta":
                        if not started:
                            started = True
                            yield {"type": "text-start", "id": text_id, "metadata": None}  # type: ignore[misc]
                        yield {"type": "text-delta", "id": text_id, "text": str(ev.get("text", ""))}  # type: ignore[misc]
                        continue
                    yield ev
            finally:
                if started:
                    yield {"type": "text-end", "id": text_id}  # type: ignore[misc]
                task.cancel()

        return AgentReplyStream(_gen(), info_future)
