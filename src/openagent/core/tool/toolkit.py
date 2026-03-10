from __future__ import annotations

"""
ToolkitAdapter：工具注册与执行中枢（对应 Agent.md 的 Tool 模块）。

核心能力：
- register_tool：注册工具（名称、描述、schema、group、危险标记）
- register_middleware：注册中间件（权限/日志/重试等）
- execute：按 tool-call 执行工具，串起中间件链，返回 ToolResult

设计取向：
- 参考 OpenCode/AgentScope 的“tool + middleware”模式
- 仅依赖标准库，方便在受限环境中运行与二次开发
"""

import asyncio
from collections.abc import Awaitable
from typing import Any

from ..id import new_id
from ..types import ToolCall, ToolResult, ToolSchema
from .middleware import Middleware
from .registry import RegisteredTool, ToolFunc


class ToolNotFoundError(KeyError):
    pass


class ToolkitAdapter:
    def __init__(self) -> None:
        # 工具表：name -> (schema, func)
        self._tools: dict[str, RegisteredTool] = {}
        # 中间件链：按注册顺序执行（内部会反向组装为“洋葱模型”）
        self._middleware: list[Middleware] = []

    def register_tool(
        self,
        name: str,
        func: ToolFunc,
        description: str = "",
        schema: dict[str, Any] | None = None,
        group: str = "default",
        dangerous: bool = False,
    ) -> None:
        # 注册工具：schema 仅用于“告知模型/校验输入”（本实现不做强校验）
        self._tools[name] = RegisteredTool(
            schema=ToolSchema(name=name, description=description, schema=schema, group=group, dangerous=dangerous),
            func=func,
        )

    def register_middleware(self, middleware: Middleware) -> None:
        # 注册中间件：例如权限检查、日志记录、失败重试等
        self._middleware.append(middleware)

    def register_mcp(self, client: object, group: str = "mcp") -> None:
        # Placeholder for MCP tool registration.
        # Expected behavior: inspect MCP client tool schemas and register them into this toolkit.
        raise NotImplementedError("MCP integration is not implemented yet")

    def get_tools_by_group(self, groups: list[str]) -> list[ToolSchema]:
        allowed = set(groups)
        return [t.schema for t in self._tools.values() if t.schema.group in allowed]

    def get_all_tools(self) -> list[ToolSchema]:
        return [t.schema for t in self._tools.values()]

    async def execute(
        self,
        *,
        name: str,
        input: dict[str, Any],
        call_id: str | None = None,
        context: dict[str, Any] | None = None,
    ) -> ToolResult:
        if name not in self._tools:
            raise ToolNotFoundError(name)

        # call_id 由模型侧给出或本地生成；用于把 tool-result 回填到正确的 tool-call
        call = ToolCall(name=name, input=input, call_id=call_id or new_id("toolcall"))
        # context 由 AgentLoop 提供：例如 session_root、memory 等
        ctx = context or {}

        async def _invoke(c: ToolCall) -> ToolResult:
            tool = self._tools[c.name]
            try:
                res = tool.func(c.input, ctx)
                if asyncio.iscoroutine(res) or isinstance(res, Awaitable):
                    out = await res  # type: ignore[assignment]
                else:
                    out = res  # type: ignore[assignment]
                return ToolResult(call_id=c.call_id, output=str(out), error=None, metadata={})
            except Exception as e:  # noqa: BLE001
                return ToolResult(call_id=c.call_id, output="", error=str(e), metadata={})

        # 组装“洋葱模型”中间件链：最后一个 handler 是真正执行工具的 _invoke
        handler = _invoke
        for mw in reversed(self._middleware):
            nxt = handler

            async def handler(c: ToolCall, mw=mw, nxt=nxt):  # type: ignore[misc]
                return await mw(c, nxt, ctx)

        return await handler(call)
