from __future__ import annotations

from typing import Any

from ..toolkit import ToolkitAdapter


def register_web_tools(toolkit: ToolkitAdapter) -> None:
    async def web_fetch(params: dict[str, Any], ctx: dict[str, Any]) -> str:  # pragma: no cover
        # Network access is environment-dependent; keep as a stub by default.
        raise RuntimeError("web_fetch is not enabled in this environment")

    async def web_search(params: dict[str, Any], ctx: dict[str, Any]) -> str:  # pragma: no cover
        raise RuntimeError("web_search is not enabled in this environment")

    toolkit.register_tool(
        "web_fetch",
        web_fetch,
        description="Fetch a URL over the network (may be disabled).",
        schema={
            "type": "object",
            "properties": {"url": {"type": "string"}, "method": {"type": "string"}, "headers": {"type": "object"}},
            "required": ["url"],
        },
        group="web",
        dangerous=True,
    )
    toolkit.register_tool(
        "web_search",
        web_search,
        description="Search the web (may be disabled).",
        schema={"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]},
        group="web",
        dangerous=True,
    )

