from __future__ import annotations

"""Search tools (`code_search`)."""

import fnmatch
import os
from dataclasses import dataclass, field
from pathlib import Path

from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry
from ..utils import resolve_optional_path

MAX_HITS = 200


@dataclass
class CodeSearchParameters:
    query: str = field(metadata={"description": "Literal substring to search for"})
    glob: str = field(default="*", metadata={"description": "Filename glob filter"})
    path: str | None = field(default=None, metadata={"description": "Search root, defaults to session_root"})


async def code_search_tool(args: CodeSearchParameters, ctx: ToolContext) -> ToolOutput:
    root = ctx.session_root.resolve()
    base = resolve_optional_path(root, args.path)
    hits: list[str] = []

    for dirpath, _dirnames, filenames in os.walk(base, onerror=lambda _e: None):
        for fn in filenames:
            if not fnmatch.fnmatch(fn, args.glob):
                continue
            p = Path(dirpath) / fn
            try:
                content = p.read_text(encoding="utf-8", errors="ignore")
            except OSError:
                continue
            for idx, line in enumerate(content.splitlines(), start=1):
                if args.query in line:
                    hits.append(f"{p}:{idx}:{line}")
                    if len(hits) >= MAX_HITS:
                        return ToolOutput(
                            title=str(base.relative_to(root)),
                            output="\n".join(hits),
                            metadata={"count": len(hits)},
                            truncated=True,
                        )

    return ToolOutput(title=str(base.relative_to(root)), output="\n".join(hits), metadata={"count": len(hits)})


def register(registry: ToolRegistry) -> None:
    registry.define_tool(tool_id="code_search", parameters=CodeSearchParameters, description_md="code_search.md", group="search", dangerous=False)(
        code_search_tool
    )


__all__ = ["register"]
