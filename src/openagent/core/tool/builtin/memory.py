from __future__ import annotations

import json
from typing import Any

from ....adapter.memory_adapter import MemoryAdapter
from ..toolkit import ToolkitAdapter


def register_memory_tools(toolkit: ToolkitAdapter) -> None:
    async def memory_read(params: dict[str, Any], ctx: dict[str, Any]) -> str:
        key = str(params["key"])
        mem: MemoryAdapter | None = ctx.get("memory")
        if mem is None:
            raise RuntimeError("No memory adapter in tool context")
        return json.dumps(mem.read(key), ensure_ascii=False)

    async def memory_write(params: dict[str, Any], ctx: dict[str, Any]) -> str:
        key = str(params["key"])
        value = params.get("value")
        mem: MemoryAdapter | None = ctx.get("memory")
        if mem is None:
            raise RuntimeError("No memory adapter in tool context")
        mem.write(key, value)
        return "ok"

    toolkit.register_tool(
        "memory_read",
        memory_read,
        description="Read a value from agent memory by key.",
        schema={"type": "object", "properties": {"key": {"type": "string"}}, "required": ["key"]},
        group="memory",
        dangerous=False,
    )
    toolkit.register_tool(
        "memory_write",
        memory_write,
        description="Write a value to agent memory by key.",
        schema={
            "type": "object",
            "properties": {"key": {"type": "string"}, "value": {}},
            "required": ["key", "value"],
        },
        group="memory",
        dangerous=False,
    )

