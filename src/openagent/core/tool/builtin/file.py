from __future__ import annotations

"""
File tools (read/write/edit/glob/grep/ls).

- 所有文件/目录操作都强制限制在 session_root 下（防止越界访问）
- 工具描述从同目录的 Markdown 文件加载（例如 read.md/ls.md）
- 输出截断由 ToolkitAdapter 统一处理；工具本体只返回“完整输出”
"""

import fnmatch
import glob as globlib
import os
import re
from dataclasses import dataclass, field
from pathlib import Path

from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry
from ..utils import ensure_within_root, resolve_optional_path, resolve_path_in_root


# ---------------------------------------------------------------------------
# Parameters (dataclass -> JSON Schema)
# ---------------------------------------------------------------------------


@dataclass
class ReadParameters:
    """read 工具参数。"""

    file_path: str = field(metadata={"description": "要读取的文件路径（相对 session_root 或绝对路径）"})
    offset: int = field(default=0, metadata={"description": "起始行号（从 0 开始）"})
    limit: int = field(default=2000, metadata={"description": "最大读取行数"})


@dataclass
class WriteParameters:
    """write 工具参数。"""

    file_path: str = field(metadata={"description": "要写入的文件路径（相对 session_root 或绝对路径）"})
    content: str = field(metadata={"description": "要写入的内容（UTF-8 文本）"})


@dataclass
class EditParameters:
    """edit 工具参数。"""

    file_path: str = field(metadata={"description": "要编辑的文件路径（相对 session_root 或绝对路径）"})
    old_string: str = field(metadata={"description": "要替换的字符串（首次出现）"})
    new_string: str = field(metadata={"description": "替换后的字符串"})


@dataclass
class GlobParameters:
    """glob 工具参数。"""

    pattern: str = field(metadata={"description": "glob 模式（支持 ** 递归，例如 **/*.py）"})
    path: str | None = field(default=None, metadata={"description": "搜索起始目录（默认 session_root）"})


@dataclass
class GrepParameters:
    """grep 工具参数。"""

    pattern: str = field(metadata={"description": "正则表达式（re）"})
    path: str | None = field(default=None, metadata={"description": "搜索起始目录（默认 session_root）"})
    glob: str = field(default="*", metadata={"description": "限制搜索的文件名 glob（默认 *）"})


@dataclass
class LsParameters:
    """ls 工具参数。"""

    path: str | None = field(default=None, metadata={"description": "要列出的目录路径（默认 session_root）"})
    ignore: list[str] | None = field(default=None, metadata={"description": "要忽略的 glob 模式列表（基于文件名匹配）"})


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _read_lines(path: Path, offset: int, limit: int) -> list[str]:
    lines = path.read_text(encoding="utf-8").splitlines()
    slice_ = lines[offset : offset + limit]
    return [f"{i + offset + 1:05d}| {line}" for i, line in enumerate(slice_)]


def _write_text(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def _replace_first(text: str, old: str, new: str) -> str:
    if old not in text:
        raise ValueError("old_string not found in file")
    return text.replace(old, new, 1)


def _iter_files(base: Path) -> list[Path]:
    files: list[Path] = []
    for dirpath, _dirnames, filenames in os.walk(base, onerror=lambda _e: None):
        for fn in filenames:
            files.append(Path(dirpath) / fn)
    return files


def _ls_entries(base: Path, *, ignore: list[str] | None) -> list[str]:
    try:
        entries = list(base.iterdir())
    except OSError:
        entries = []

    ignore = ignore or []
    filtered: list[Path] = []
    for p in entries:
        if any(fnmatch.fnmatch(p.name, pat) for pat in ignore):
            continue
        filtered.append(p)

    items: list[str] = []
    for p in sorted(filtered, key=lambda x: (not x.is_dir(), x.name.lower())):
        kind = "d" if p.is_dir() else "-"
        try:
            size = p.stat().st_size if p.is_file() else 0
        except OSError:
            size = 0
        items.append(f"{kind} {size:>10} {p.name}")
    return items


def _glob_paths(root: Path, base: Path, pattern: str) -> list[Path]:
    raw = globlib.glob(str(base / pattern), recursive=True)
    out: list[Path] = []
    for s in raw:
        p = Path(s)
        try:
            p = ensure_within_root(root, p)
        except PermissionError:
            continue
        out.append(p)

    def _mtime(p: Path) -> float:
        try:
            return p.stat().st_mtime
        except OSError:
            return 0.0

    out.sort(key=_mtime, reverse=True)
    return out


def _grep_files(root: Path, base: Path, pattern: str, glob_pat: str) -> list[str]:
    regex = re.compile(pattern)
    hits: list[str] = []
    for p in _iter_files(base):
        if not fnmatch.fnmatch(p.name, glob_pat):
            continue
        try:
            p = ensure_within_root(root, p)
        except PermissionError:
            continue
        try:
            content = p.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        for idx, line in enumerate(content.splitlines(), start=1):
            if regex.search(line):
                hits.append(f"{p}:{idx}:{line}")
                if len(hits) >= 500:
                    return hits
    return hits


# ---------------------------------------------------------------------------
# Tools
# ---------------------------------------------------------------------------


async def read_tool(args: ReadParameters, ctx: ToolContext) -> ToolOutput:
    root = ctx.session_root.resolve()
    target = resolve_path_in_root(root, args.file_path)
    out_lines = _read_lines(target, args.offset, args.limit)
    output = "<file>\n" + "\n".join(out_lines) + "\n</file>"
    return ToolOutput(title=str(target.relative_to(root)), output=output, metadata={})


async def write_tool(args: WriteParameters, ctx: ToolContext) -> ToolOutput:
    root = ctx.session_root.resolve()
    target = resolve_path_in_root(root, args.file_path)
    _write_text(target, args.content)
    return ToolOutput(title=str(target.relative_to(root)), output=f"Wrote {len(args.content)} chars to {target}", metadata={})


async def edit_tool(args: EditParameters, ctx: ToolContext) -> ToolOutput:
    root = ctx.session_root.resolve()
    target = resolve_path_in_root(root, args.file_path)
    text = target.read_text(encoding="utf-8")
    new_text = _replace_first(text, args.old_string, args.new_string)
    target.write_text(new_text, encoding="utf-8")
    return ToolOutput(title=str(target.relative_to(root)), output=f"Edited {target}", metadata={})


async def glob_tool(args: GlobParameters, ctx: ToolContext) -> ToolOutput:
    root = ctx.session_root.resolve()
    base = resolve_optional_path(root, args.path)
    matches = _glob_paths(root, base, args.pattern)
    return ToolOutput(title=str(base.relative_to(root)), output="\n".join(str(p) for p in matches), metadata={"count": len(matches)})


async def grep_tool(args: GrepParameters, ctx: ToolContext) -> ToolOutput:
    root = ctx.session_root.resolve()
    base = resolve_optional_path(root, args.path)
    hits = _grep_files(root, base, args.pattern, args.glob)
    return ToolOutput(title=str(base.relative_to(root)), output="\n".join(hits), metadata={"count": len(hits)})


async def ls_tool(args: LsParameters, ctx: ToolContext) -> ToolOutput:
    root = ctx.session_root.resolve()
    base = resolve_optional_path(root, args.path)
    items = _ls_entries(base, ignore=args.ignore)
    title = "."
    try:
        title = str(base.relative_to(root))
    except Exception:
        title = str(base)
    return ToolOutput(title=title, output="\n".join(items), metadata={"count": len(items)})


def register(registry: ToolRegistry) -> None:
    # 文件类工具：默认分组 file；write/edit 属于“危险工具”
    registry.define_tool(tool_id="read", parameters=ReadParameters, description_md="read.md", group="file", dangerous=False)(read_tool)
    registry.define_tool(tool_id="write", parameters=WriteParameters, description_md="write.md", group="file", dangerous=True)(write_tool)
    registry.define_tool(tool_id="edit", parameters=EditParameters, description_md="edit.md", group="file", dangerous=True)(edit_tool)
    registry.define_tool(tool_id="glob", parameters=GlobParameters, description_md="glob.md", group="file", dangerous=False)(glob_tool)
    registry.define_tool(tool_id="grep", parameters=GrepParameters, description_md="grep.md", group="file", dangerous=False)(grep_tool)
    registry.define_tool(tool_id="ls", parameters=LsParameters, description_md="ls.md", group="file", dangerous=False)(ls_tool)


__all__ = ["register"]

