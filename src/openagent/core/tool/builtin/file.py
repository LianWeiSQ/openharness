from __future__ import annotations

import fnmatch
import os
from pathlib import Path
from typing import Any

from ..toolkit import ToolkitAdapter


def _ensure_within_root(root: Path, target: Path) -> Path:
    root_resolved = root.resolve()
    target_resolved = target.resolve()
    if root_resolved not in target_resolved.parents and root_resolved != target_resolved:
        raise PermissionError(f"Path escapes session root: {target}")
    return target_resolved


def register_file_tools(toolkit: ToolkitAdapter) -> None:
    async def read_tool(params: dict[str, Any], ctx: dict[str, Any]) -> str:
        file_path = str(params["file_path"])
        offset = int(params.get("offset", 0))
        limit = int(params.get("limit", 2000))
        root = Path(str(ctx.get("session_root") or os.getcwd()))
        target = Path(file_path)
        if not target.is_absolute():
            target = root / target
        target = _ensure_within_root(root, target)
        lines = target.read_text(encoding="utf-8").splitlines()
        slice_ = lines[offset : offset + limit]
        out_lines = [f"{i + offset + 1:05d}| {line}" for i, line in enumerate(slice_)]
        return "<file>\n" + "\n".join(out_lines) + "\n</file>"

    async def write_tool(params: dict[str, Any], ctx: dict[str, Any]) -> str:
        file_path = str(params["file_path"])
        content = str(params["content"])
        root = Path(str(ctx.get("session_root") or os.getcwd()))
        target = Path(file_path)
        if not target.is_absolute():
            target = root / target
        target = _ensure_within_root(root, target)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
        return f"Wrote {len(content)} chars to {target}"

    async def edit_tool(params: dict[str, Any], ctx: dict[str, Any]) -> str:
        file_path = str(params["file_path"])
        old = str(params["old_string"])
        new = str(params["new_string"])
        root = Path(str(ctx.get("session_root") or os.getcwd()))
        target = Path(file_path)
        if not target.is_absolute():
            target = root / target
        target = _ensure_within_root(root, target)
        text = target.read_text(encoding="utf-8")
        if old not in text:
            raise ValueError("old_string not found in file")
        target.write_text(text.replace(old, new, 1), encoding="utf-8")
        return f"Edited {target}"

    async def glob_tool(params: dict[str, Any], ctx: dict[str, Any]) -> str:
        pattern = str(params["pattern"])
        base = Path(str(params.get("path") or ctx.get("session_root") or os.getcwd()))
        root = Path(str(ctx.get("session_root") or os.getcwd()))
        base = _ensure_within_root(root, base)
        matches: list[str] = []
        for dirpath, _dirnames, filenames in os.walk(base, onerror=lambda _e: None):
            for fn in filenames:
                if fnmatch.fnmatch(fn, pattern):
                    matches.append(str(Path(dirpath) / fn))
        return "\n".join(matches)

    async def grep_tool(params: dict[str, Any], ctx: dict[str, Any]) -> str:
        pattern = str(params["pattern"])
        base = Path(str(params.get("path") or ctx.get("session_root") or os.getcwd()))
        glob_pat = str(params.get("glob") or "*")
        root = Path(str(ctx.get("session_root") or os.getcwd()))
        base = _ensure_within_root(root, base)
        hits: list[str] = []
        for dirpath, _dirnames, filenames in os.walk(base, onerror=lambda _e: None):
            for fn in filenames:
                if not fnmatch.fnmatch(fn, glob_pat):
                    continue
                p = Path(dirpath) / fn
                try:
                    content = p.read_text(encoding="utf-8", errors="ignore")
                except OSError:
                    continue
                for idx, line in enumerate(content.splitlines(), start=1):
                    if pattern in line:
                        hits.append(f"{p}:{idx}:{line}")
        return "\n".join(hits)

    async def ls_tool(params: dict[str, Any], ctx: dict[str, Any]) -> str:
        path = str(params.get("path") or ctx.get("session_root") or os.getcwd())
        root = Path(str(ctx.get("session_root") or os.getcwd()))
        base = _ensure_within_root(root, Path(path))
        items: list[str] = []
        try:
            entries = list(base.iterdir())
        except OSError:
            entries = []
        for p in sorted(entries, key=lambda x: (not x.is_dir(), x.name.lower())):
            kind = "d" if p.is_dir() else "-"
            try:
                size = p.stat().st_size if p.is_file() else 0
            except OSError:
                size = 0
            items.append(f"{kind} {size:>10} {p.name}")
        return "\n".join(items)

    toolkit.register_tool(
        "read",
        read_tool,
        description="Read a UTF-8 text file with optional offset/limit (line-based).",
        schema={
            "type": "object",
            "properties": {"file_path": {"type": "string"}, "offset": {"type": "integer"}, "limit": {"type": "integer"}},
            "required": ["file_path"],
        },
        group="file",
        dangerous=False,
    )
    toolkit.register_tool(
        "write",
        write_tool,
        description="Write a UTF-8 text file (overwrites).",
        schema={
            "type": "object",
            "properties": {"file_path": {"type": "string"}, "content": {"type": "string"}},
            "required": ["file_path", "content"],
        },
        group="file",
        dangerous=True,
    )
    toolkit.register_tool(
        "edit",
        edit_tool,
        description="Replace the first occurrence of old_string with new_string in a file.",
        schema={
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
            },
            "required": ["file_path", "old_string", "new_string"],
        },
        group="file",
        dangerous=True,
    )
    toolkit.register_tool(
        "glob",
        glob_tool,
        description="Find files by name glob under a base path.",
        schema={"type": "object", "properties": {"pattern": {"type": "string"}, "path": {"type": "string"}}, "required": ["pattern"]},
        group="file",
        dangerous=False,
    )
    toolkit.register_tool(
        "grep",
        grep_tool,
        description="Search for a substring in files under a base path.",
        schema={
            "type": "object",
            "properties": {"pattern": {"type": "string"}, "path": {"type": "string"}, "glob": {"type": "string"}},
            "required": ["pattern"],
        },
        group="file",
        dangerous=False,
    )
    toolkit.register_tool(
        "ls",
        ls_tool,
        description="List directory entries under a base path.",
        schema={"type": "object", "properties": {"path": {"type": "string"}}, "required": []},
        group="file",
        dangerous=False,
    )
