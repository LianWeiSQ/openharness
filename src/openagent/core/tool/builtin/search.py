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
MAX_PREVIEW_HITS = 20
MAX_LINE_CHARS = 240


@dataclass
class CodeSearchParameters:
    query: str = field(metadata={"description": "Literal substring to search for"})
    glob: str = field(default="*", metadata={"description": "Filename glob filter"})
    path: str | None = field(default=None, metadata={"description": "Search root, defaults to session_root"})


async def code_search_tool(args: CodeSearchParameters, ctx: ToolContext) -> ToolOutput:
    root = ctx.session_root.resolve()
    base = resolve_optional_path(root, args.path)
    hits: list[str] = []
    preview_hits: list[str] = []

    for dirpath, _dirnames, filenames in os.walk(base, onerror=lambda _e: None):
        for filename in filenames:
            if not fnmatch.fnmatch(filename, args.glob):
                continue
            path = Path(dirpath) / filename
            try:
                content = path.read_text(encoding="utf-8", errors="ignore")
            except OSError:
                continue
            for index, line in enumerate(content.splitlines(), start=1):
                if args.query not in line:
                    continue
                clipped_line = line if len(line) <= MAX_LINE_CHARS else line[:MAX_LINE_CHARS] + "..."
                hit = f"{path}:{index}:{clipped_line}"
                hits.append(hit)
                if len(preview_hits) < MAX_PREVIEW_HITS:
                    preview_hits.append(hit)
                if len(hits) >= MAX_HITS:
                    return ToolOutput(
                        title=str(base.relative_to(root)),
                        output="\n".join(hits),
                        metadata={
                            "count": len(hits),
                            "returned_count": len(hits),
                            "preview": "\n".join(preview_hits),
                        },
                        truncated=True,
                    )

    return ToolOutput(
        title=str(base.relative_to(root)),
        output="\n".join(hits),
        metadata={
            "count": len(hits),
            "returned_count": len(hits),
            "preview": "\n".join(preview_hits),
        },
    )


def register(registry: ToolRegistry) -> None:
    registry.define_tool(tool_id="code_search", parameters=CodeSearchParameters, description_md="code_search.md", group="search", dangerous=False)(
        code_search_tool
    )


__all__ = ["register"]
