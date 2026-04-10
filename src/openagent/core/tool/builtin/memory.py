from __future__ import annotations

"""
Memory tools (memory_read/memory_write).

- MemoryAdapter 由 AgentLoop 注入到 ToolContext.extra["memory"]
- 工具本体不关心存储实现，只依赖 read/write 接口
"""

import json
from dataclasses import dataclass, field
from typing import Any

from ....adapter.memory_adapter import MemoryAdapter
from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry


@dataclass
class MemoryReadParameters:
    key: str = field(metadata={"description": "要读取的 key"})


@dataclass
class MemoryWriteParameters:
    key: str = field(metadata={"description": "要写入的 key"})
    value: Any = field(metadata={"description": "要写入的值（任意 JSON 结构）"})


async def memory_read_tool(args: MemoryReadParameters, ctx: ToolContext) -> ToolOutput:
    mem = ctx.extra.get("memory")
    if not isinstance(mem, MemoryAdapter):
        raise RuntimeError("No memory adapter in tool context")
    return ToolOutput(title=args.key, output=json.dumps(mem.read(args.key), ensure_ascii=False), metadata={})


async def memory_write_tool(args: MemoryWriteParameters, ctx: ToolContext) -> ToolOutput:
    mem = ctx.extra.get("memory")
    if not isinstance(mem, MemoryAdapter):
        raise RuntimeError("No memory adapter in tool context")
    mem.write(args.key, args.value)
    return ToolOutput(title=args.key, output="ok", metadata={})


def register(registry: ToolRegistry) -> None:
    registry.define_tool(tool_id="memory_read", parameters=MemoryReadParameters, description_md="memory_read.md", group="memory", dangerous=False, execution_scope="agnostic")(
        memory_read_tool
    )
    registry.define_tool(tool_id="memory_write", parameters=MemoryWriteParameters, description_md="memory_write.md", group="memory", dangerous=False, execution_scope="agnostic")(
        memory_write_tool
    )


__all__ = ["register"]

