from __future__ import annotations

"""
AgentLoop：OpenAgent 的“主循环引擎”。

该模块对应 Loop Layer，负责把一次用户输入驱动成多步执行：
- step-start：创建文件快照（用于后续 patch）
- 调用 AgentAdapter.reply_stream：获取模型的流式事件（text/tool-call/finish）
- 执行工具：按 tool-call 调用 ToolkitAdapter，并回传 tool-result
- patch：计算工作区变更（diff）
- step-finish：统计 tokens/cost/finish_reason，并根据循环控制决定 continue/stop

实现侧重点：
- 对齐 OpenCode 的 SessionProcessor 思路，但用 Python 标准库实现最小闭环
- “权限/工具/快照”通过组合注入，便于替换和扩展
"""

import asyncio
import json
from collections.abc import AsyncIterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ..agent.base import BaseAgent
from ..permission.manager import PermissionAskRequiredError, PermissionDeniedError, PermissionManager
from ..permission.ruleset import PermissionRuleset
from ...adapter.memory_adapter import MemoryAdapter
from ..tool.middleware import logging_middleware, permission_middleware
from ..tool.toolkit import ToolkitAdapter
from ..types import ChatMessage, FinishReason, StreamEvent, ToolSchema, Usage
from .doom_loop import DoomLoopDetector
from .snapshot import SnapshotManager


@dataclass(slots=True)
class AgentLoopConfig:
    max_steps: int = 50 # 最大执行步数（防止无限循环）
    doom_loop_threshold: int = 3 # 连续相同 tool-call 触发 doom-loop 检测
    max_retry: int = 2 # Provider 侧失败重试次数（仅在“尚未输出任何流事件”时重试）
    retry_base_delay_s: float = 1.0 # 重试基础延时（指数退避）


class AgentLoop:
    def __init__(
        self,
        *,
        agent: BaseAgent,
        session,
        permission_manager: PermissionManager,
        toolkit: ToolkitAdapter | None = None,
        snapshot_manager: SnapshotManager | None = None,
        doom_loop_detector: DoomLoopDetector | None = None,
        config: AgentLoopConfig | None = None,
    ) -> None:
        self.agent = agent
        self.session = session
        self.permission_manager = permission_manager
        self.config = config or AgentLoopConfig(max_steps=agent.config.max_steps)
        self.snapshot_manager = snapshot_manager or SnapshotManager()
        self.doom_loop_detector = doom_loop_detector or DoomLoopDetector(self.config.doom_loop_threshold)
        self.toolkit = toolkit or ToolkitAdapter()
        # 记忆适配器：工具可通过 memory_read/memory_write 访问
        self.memory = MemoryAdapter()
        # 工具调用日志：便于调试/观测（可在上层接入更正式的 event bus）
        self.tool_log: list[dict[str, Any]] = []
        self._init_tools()

    def _tools_for_agent(self) -> list[ToolSchema]:
        """
        根据 AgentConfig.tools 决定“暴露给模型”的工具集合。
        - 工具“是否可见”与“是否允许执行”是两件事：
          - 可见性：决定模型能不能生成 tool-call（避免无意义/危险调用）
          - 允许执行：由 PermissionManager 在真正 execute 前做最终裁决
        - permission="NONE" 时直接不暴露任何工具，避免模型生成 tool-call 后必然被拒绝。
        """

        if self.agent.config.permission == "NONE":
            return []

        tools = self.toolkit.get_all_tools()
        allow = self.agent.config.tools
        if allow == "all":
            return tools
        if allow == "readonly":
            # v1 约定：readonly 仅暴露“只读文件工具”，用于安全的目录/文本探索
            allowed_names = {"read", "glob", "grep", "ls"}
            return [t for t in tools if t.name in allowed_names]
        # allowlist：仅暴露指定名称的工具
        if isinstance(allow, list):
            allowed_names = set(allow)
            return [t for t in tools if t.name in allowed_names]
        return tools

    def _init_tools(self) -> None:
        # 中间件链：权限检查 → 记录日志 →（执行工具本体）
        self.toolkit.register_middleware(permission_middleware(self.permission_manager))
        self.toolkit.register_middleware(logging_middleware(self.tool_log))
        # 内置工具注册（统一由 ToolRegistry 管理）
        self.toolkit.load_builtin()

        # 可选：加载外部插件工具
        tool_paths = self.agent.config.options.get("tool_paths")
        if isinstance(tool_paths, list) and all(isinstance(x, str) for x in tool_paths):
            self.toolkit.load_plugins(tool_paths=tool_paths, base_dir=Path(self.session.directory))

    async def run(self, user_text: str) -> AsyncIterator[StreamEvent]:
        # 1) 应用 Agent 的权限规则集（FULL/READONLY/PLAN_ONLY/NONE）
        self.permission_manager.set_ruleset(PermissionRuleset[self.agent.config.permission])
        
        # 2) 记录用户消息进入会话（后续会作为上下文传入模型）
        self.session.add(ChatMessage(role="user", content=user_text))
        steps = 0
        while steps < self.config.max_steps:
            steps += 1
            # step-start：创建快照，用于 step 完成后生成 patch（文件 diff）
            snapshot_id = self.snapshot_manager.track(Path(self.session.directory))
            yield {"type": "step-start", "snapshot_id": snapshot_id}  # type: ignore[misc]
            tools = self._tools_for_agent()

            # Stream a model step (with limited retry for "no output yet" failures).
            attempt = 0
            # 本 step 的 assistant 文本输出（用于写回 Session，保证多步推理上下文正确）
            assistant_text_chunks: list[str] = []
            while True:
                attempt += 1
                yielded = False
                adapter = self.agent.adapter()
                stream = adapter.reply_stream(
                    system=self.agent.system_prompt,
                    messages=list(self.session.messages),
                    tools=tools,
                )
                try:
                    assistant_text_chunks = []
                    async for ev in stream:
                        yielded = True
                        if ev["type"] == "text-delta":
                            # 我们在 loop 层把本次模型输出的文本拼起来，
                            # 并在 step 结束后写回 Session.messages（assistant role）。
                            assistant_text_chunks.append(ev["text"])
                        # 直接把模型流事件透传出去（上层可实时渲染）
                        yield ev
                    info = await stream.info()
                    break
                except Exception as e:  # noqa: BLE001
                    if attempt > self.config.max_retry or yielded:
                        yield {"type": "error", "error": str(e)}  # type: ignore[misc]
                        return
                    # 指数退避（避免瞬时网络/服务抖动导致连续失败）
                    await asyncio.sleep(self.config.retry_base_delay_s * (2 ** (attempt - 1)))

            # 将本 step 的 assistant 输出（以及 tool_calls）写回会话上下文。
            # 这样下一轮模型就能看到完整对话链路：user -> assistant(tool_calls) -> tool -> assistant...
            assistant_content = "".join(assistant_text_chunks)
            if assistant_content or info.tool_calls:
                metadata: dict[str, Any] = {}
                if info.tool_calls:
                    # OpenAI-compatible 结构：arguments 需要是 JSON 字符串
                    metadata["tool_calls"] = [
                        {
                            "id": c.call_id,
                            "type": "function",
                            "function": {"name": c.name, "arguments": json.dumps(c.input, ensure_ascii=False)},
                        }
                        for c in info.tool_calls
                    ]
                self.session.add(ChatMessage(role="assistant", content=assistant_content, metadata=metadata))

            blocked = False
            for call in info.tool_calls:
                # doom-loop：连续多次执行完全相同的工具调用，视为“无进展循环”
                if self.doom_loop_detector.record(call):
                    try:
                        await self.permission_manager.check(
                            {"name": "doom_loop", "input": {"tool": call.name, "input": call.input}}
                        )
                    except Exception:
                        blocked = True

                try:
                    # 工具执行上下文：
                    # - session_root：限制文件/命令只能在会话目录内操作
                    # - memory：提供 memory_read/memory_write 的底层存储
                    result = await self.toolkit.execute(
                        name=call.name,
                        input=call.input,
                        call_id=call.call_id,
                        context={
                            "session_id": self.session.id,
                            "session_root": str(self.session.directory),
                            "memory": self.memory,
                        },
                    )
                except (PermissionDeniedError, PermissionAskRequiredError) as e:
                    blocked = True
                    yield {"type": "tool-result", "call_id": call.call_id, "output": "", "error": str(e), "metadata": None}  # type: ignore[misc]
                    continue
                except Exception as e:  # noqa: BLE001
                    yield {"type": "tool-result", "call_id": call.call_id, "output": "", "error": str(e), "metadata": None}  # type: ignore[misc]
                    blocked = True
                    continue

                yield {
                    "type": "tool-result",
                    "call_id": result.call_id,
                    "output": result.output,
                    "error": result.error,
                    "metadata": result.metadata,
                }  # type: ignore[misc]
                # 将工具结果写回会话上下文（供下一轮模型继续推理/总结）
                self.session.add(
                    ChatMessage(
                        role="tool",
                        name=call.name,
                        tool_call_id=call.call_id,
                        content=result.output if not result.error else f"ERROR: {result.error}",
                    )
                )
                if result.error:
                    blocked = True

            # 计算 patch：如果本 step 有文件变更，输出 patch 事件
            patch = self.snapshot_manager.patch(snapshot_id)
            if patch.get("files"):
                yield {"type": "patch", "snapshot_id": snapshot_id, "hash": patch["hash"], "files": patch["files"]}  # type: ignore[misc]
            usage: Usage = info.usage
            finish_reason: FinishReason = info.finish_reason
            if info.tool_calls and finish_reason == "unknown":
                finish_reason = "tool_call"
            # step-finish：输出本步的 usage / cost / finish_reason
            yield {
                "type": "step-finish",
                "tokens": {"input": usage.input_tokens, "output": usage.output_tokens},
                "cost": usage.cost,
                "finish_reason": finish_reason,
            }  # type: ignore[misc]
            if blocked:
                # 工具被拒绝/失败时，默认中断（也可以在此做 “continue_loop_on_deny” 等实验配置）
                return
            if info.tool_calls:
                # 如果本步产生工具调用，通常需要再跑一轮，让模型读取工具结果后继续
                continue
            if finish_reason == "stop":
                return
        yield {"type": "error", "error": "max_steps exceeded"}  # type: ignore[misc]
