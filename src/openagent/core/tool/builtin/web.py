from __future__ import annotations

"""
Web tools (web_fetch/web_search).

中文说明：
- 该仓库默认运行环境可能禁网，因此这里默认返回“未启用”
- 如果你要开启，可在此处实现真实网络请求，并在 PermissionRuleset 中做更严格控制
"""

from dataclasses import dataclass, field
from typing import Any

from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry


@dataclass
class WebFetchParameters:
    url: str = field(metadata={"description": "要抓取的 URL"})
    method: str = field(default="GET", metadata={"description": "HTTP 方法（默认 GET）"})
    headers: dict[str, Any] | None = field(default=None, metadata={"description": "请求头（可选）"})


@dataclass
class WebSearchParameters:
    query: str = field(metadata={"description": "搜索关键词"})


async def web_fetch_tool(_args: WebFetchParameters, _ctx: ToolContext) -> ToolOutput:
    raise RuntimeError("web_fetch is not enabled in this environment")


async def web_search_tool(_args: WebSearchParameters, _ctx: ToolContext) -> ToolOutput:
    raise RuntimeError("web_search is not enabled in this environment")


def register(registry: ToolRegistry) -> None:
    registry.define_tool(tool_id="web_fetch", parameters=WebFetchParameters, description_md="web_fetch.md", group="web", dangerous=True)(web_fetch_tool)
    registry.define_tool(tool_id="web_search", parameters=WebSearchParameters, description_md="web_search.md", group="web", dangerous=True)(web_search_tool)


__all__ = ["register"]

