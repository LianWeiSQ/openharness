from __future__ import annotations

"""
AgentScopeUniversalAgent：OpenAgent 的“AgentScope 外壳”Agent。

定位：
- 仍然使用现有 AgentLoop（快照/patch/step-finish 等语义不变）
- 底层推理与工具执行交给 AgentScope 的 ReActAgent
- 对外输出仍是 OpenAgent 的 StreamEvent（便于 UI/CLI 统一消费）

使用方式要点：
- 需要你在外部创建并共享同一个 PermissionManager + ToolkitAdapter
  （这样 AgentScope 执行工具时会复用现有权限策略与工具实现）
- agentscope 是可选依赖：未安装时，只要不实例化/运行该 Agent，就不会影响其他功能
"""

from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ...adapter.agentscope_adapter import AgentScopeAgentAdapter, AgentScopeBackend
from ...adapter.memory_adapter import MemoryAdapter
from ..permission.manager import PermissionManager
from ..tool.toolkit import ToolkitAdapter
from ..types import AgentConfig


@dataclass(slots=True)
class AgentScopeUniversalAgent:
    config: AgentConfig
    system_prompt: str
    session_root: Path
    permission_manager: PermissionManager
    toolkit: ToolkitAdapter
    memory: MemoryAdapter
    backend: AgentScopeBackend | None = None

    def adapter(self) -> Any:
        # 返回一个具备 reply_stream()/info() 的 adapter 对象，供 AgentLoop 调用
        return AgentScopeAgentAdapter(
            config=self.config,
            session_root=self.session_root,
            permission_manager=self.permission_manager,
            toolkit=self.toolkit,
            memory=self.memory,
            backend=self.backend,
        )

