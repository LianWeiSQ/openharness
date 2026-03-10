from __future__ import annotations

import fnmatch
import os
from pathlib import Path
from typing import Any

from ..toolkit import ToolkitAdapter


def register_search_tools(toolkit: ToolkitAdapter) -> None:
    async def code_search(params: dict[str, Any], ctx: dict[str, Any]) -> str:
        query = str(params["query"])
        glob_pat = str(params.get("glob") or "*")
        root = Path(str(ctx.get("session_root") or os.getcwd())).resolve()
        hits: list[str] = []
        for dirpath, _dirnames, filenames in os.walk(root, onerror=lambda _e: None):
            for fn in filenames:
                if not fnmatch.fnmatch(fn, glob_pat):
                    continue
                p = Path(dirpath) / fn
                try:
                    content = p.read_text(encoding="utf-8", errors="ignore")
                except OSError:
                    continue
                for idx, line in enumerate(content.splitlines(), start=1):
                    if query in line:
                        hits.append(f"{p}:{idx}:{line}")
                        if len(hits) >= 200:
                            return "\n".join(hits) + "\n... truncated ..."
        return "\n".join(hits)

    async def list_definitions(params: dict[str, Any], ctx: dict[str, Any]) -> str:  # pragma: no cover
        raise RuntimeError("list_definitions is not implemented yet")

    toolkit.register_tool(
        "code_search",
        code_search,
        description="Search code under the session root (substring match).",
        schema={"type": "object", "properties": {"query": {"type": "string"}, "glob": {"type": "string"}}, "required": ["query"]},
        group="search",
        dangerous=False,
    )
    toolkit.register_tool(
        "list_definitions",
        list_definitions,
        description="List code definitions (stub).",
        schema={"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]},
        group="search",
        dangerous=False,
    )
