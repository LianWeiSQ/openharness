from __future__ import annotations

"""
File tools (read/write/edit/glob/grep/ls).

- 所有文件/目录操作都强制限制在 session_root 下（防止越界访问）
- 工具描述从同目录的 Markdown 文件加载（例如 read.md/ls.md）
- 输出截断由 ToolkitAdapter 统一处理；工具本体只返回“完整输出”
"""

import difflib
import fnmatch
import glob as globlib
import os
import re
import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from ...file_context import record_file_read
from ...session.session import Session
from ..definition import ToolContext, ToolExecutionSchema, ToolOutput
from ..registry import ToolRegistry
from ..utils import ensure_within_root, resolve_optional_path, resolve_path_in_root

DEFAULT_READ_LIMIT = 2000
MAX_LINE_LENGTH = 2000
MAX_READ_BYTES = 50 * 1024
GLOB_LIMIT = 100
GREP_LIMIT = 100
LS_LIMIT = 100
DEFAULT_LS_IGNORE = [
    "node_modules/",
    "__pycache__/",
    ".git/",
    "dist/",
    "build/",
    "target/",
    "vendor/",
    ".idea/",
    ".vscode/",
    ".venv/",
    "venv/",
    "env/",
    "coverage/",
]
BINARY_EXTENSIONS = {
    ".zip",
    ".tar",
    ".gz",
    ".exe",
    ".dll",
    ".so",
    ".class",
    ".jar",
    ".war",
    ".7z",
    ".doc",
    ".docx",
    ".xls",
    ".xlsx",
    ".ppt",
    ".pptx",
    ".odt",
    ".ods",
    ".odp",
    ".bin",
    ".dat",
    ".obj",
    ".o",
    ".a",
    ".lib",
    ".wasm",
    ".pyc",
    ".pyo",
    ".pdf",
    ".png",
    ".jpg",
    ".jpeg",
    ".gif",
    ".webp",
    ".ico",
}


@dataclass
class ReadParameters:
    """read 工具参数。"""

    file_path: str = field(metadata={"description": "要读取的文件路径（相对 session_root 或绝对路径）"})
    offset: int = field(default=0, metadata={"description": "起始行号（从 0 开始）"})
    limit: int = field(default=DEFAULT_READ_LIMIT, metadata={"description": "最大读取行数"})


@dataclass
class WriteParameters:
    """write 工具参数。"""

    file_path: str = field(metadata={"description": "要写入的文件路径（相对 session_root 或绝对路径）"})
    content: str = field(metadata={"description": "要写入的内容（UTF-8 文本）"})


@dataclass
class EditParameters:
    """edit 工具参数。"""

    file_path: str = field(metadata={"description": "要编辑的文件路径（相对 session_root 或绝对路径）"})
    old_string: str = field(metadata={"description": "要替换的字符串"})
    new_string: str = field(metadata={"description": "替换后的字符串"})
    replace_all: bool = field(default=False, metadata={"description": "是否替换全部匹配项（默认 false）"})


@dataclass
class GlobParameters:
    """glob 工具参数。"""

    pattern: str = field(metadata={"description": "glob 模式（支持 ** 递归，例如 **/*.py）"})
    path: str | None = field(default=None, metadata={"description": "搜索起始目录（默认 session_root）"})


@dataclass
class GrepParameters:
    """grep 工具参数。"""

    pattern: str = field(metadata={"description": "正则表达式（re / ripgrep）"})
    path: str | None = field(default=None, metadata={"description": "搜索起始目录（默认 session_root）"})
    glob: str | None = field(default=None, metadata={"description": "兼容字段：文件名 glob 过滤，例如 *.py"})
    include: str | None = field(default=None, metadata={"description": "文件名 glob 过滤，例如 *.py 或 *.{ts,tsx}"})


@dataclass
class LsParameters:
    """ls 工具参数。"""

    path: str | None = field(default=None, metadata={"description": "要列出的目录路径（默认 session_root）"})
    ignore: list[str] | None = field(default=None, metadata={"description": "要忽略的 glob 模式列表"})



def _session_from_ctx(ctx: ToolContext) -> Session | None:
    session = ctx.extra.get("session")
    if isinstance(session, Session):
        return session
    return None



def _display_path(root: Path, target: Path) -> str:
    try:
        return str(target.relative_to(root))
    except Exception:  # noqa: BLE001
        return str(target)



def _remember_read(ctx: ToolContext, target: Path, *, content: str | bytes | None = None, source_tool: str = "read") -> None:
    session = _session_from_ctx(ctx)
    if session is not None:
        session.remember_file_read(target)
        record_file_read(
            session.metadata,
            target,
            workspace_root=ctx.session_root,
            content=content,
            source_tool=source_tool,
        )



def _require_existing_file_was_read(ctx: ToolContext, target: Path, *, action: str) -> None:
    session = _session_from_ctx(ctx)
    if session is None or not target.exists():
        return
    if not session.has_read_file(target):
        raise ValueError(f"Must read existing file before {action} it: {target}")



def _read_text(path: Path) -> str:
    if _is_binary_file(path):
        raise ValueError(f"Cannot read binary file: {path}")
    return path.read_text(encoding="utf-8")



def _is_binary_file(path: Path) -> bool:
    if path.suffix.lower() in BINARY_EXTENSIONS:
        return True
    data = path.read_bytes()[:4096]
    if not data:
        return False
    if b"\x00" in data:
        return True
    non_printable = 0
    for byte in data:
        if byte < 9 or (byte > 13 and byte < 32):
            non_printable += 1
    return non_printable / len(data) > 0.3



def _suggest_paths(target: Path) -> list[str]:
    parent = target.parent
    if not parent.exists() or not parent.is_dir():
        return []
    names = [entry.name for entry in parent.iterdir()]
    matches = difflib.get_close_matches(target.name, names, n=3, cutoff=0.3)
    return [str(parent / name) for name in matches]



def _format_read_output_from_text(text: str, *, offset: int, limit: int) -> tuple[str, str, bool]:
    lines = text.splitlines() if text else []
    total_lines = len(lines)
    start = max(offset, 0)
    max_lines = max(limit, 0)

    raw: list[str] = []
    preview_lines: list[str] = []
    bytes_used = 0
    truncated_by_bytes = False

    for index in range(start, min(total_lines, start + max_lines)):
        line = lines[index]
        if len(line) > MAX_LINE_LENGTH:
            line = line[:MAX_LINE_LENGTH] + "..."
        encoded_size = len(line.encode("utf-8")) + (1 if raw else 0)
        if bytes_used + encoded_size > MAX_READ_BYTES:
            truncated_by_bytes = True
            break
        raw.append(line)
        bytes_used += encoded_size
        if len(preview_lines) < 20:
            preview_lines.append(line)

    numbered = [f"{index + start + 1:05d}| {line}" for index, line in enumerate(raw)]
    last_read_line = start + len(raw)
    has_more_lines = total_lines > last_read_line
    truncated = truncated_by_bytes or has_more_lines

    output_lines = ["<file>"]
    output_lines.extend(numbered)
    output_lines.append("")
    if truncated_by_bytes:
        output_lines.append(f"(Output truncated at {MAX_READ_BYTES} bytes. Use 'offset' parameter to read beyond line {last_read_line})")
    elif has_more_lines:
        output_lines.append(f"(File has more lines. Use 'offset' parameter to read beyond line {last_read_line})")
    else:
        output_lines.append(f"(End of file - total {total_lines} lines)")
    output_lines.append("</file>")

    return "\n".join(output_lines), "\n".join(preview_lines), truncated


def _format_read_output(path: Path, *, offset: int, limit: int) -> tuple[str, str, bool]:
    return _format_read_output_from_text(_read_text(path), offset=offset, limit=limit)



def _write_text(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")



def _replace_text(content: str, old: str, new: str, *, replace_all: bool) -> str:
    if old == new:
        raise ValueError("old_string and new_string must be different")
    if old == "":
        return new

    count = content.count(old)
    if count == 0:
        raise ValueError("old_string not found in content")
    if count > 1 and not replace_all:
        raise ValueError(
            "old_string found multiple times and requires more code context to uniquely identify the intended match"
        )
    return content.replace(old, new) if replace_all else content.replace(old, new, 1)



def _iter_files(base: Path) -> list[Path]:
    files: list[Path] = []
    if base.is_file():
        return [base]
    for dirpath, dirnames, filenames in os.walk(base, topdown=True, onerror=lambda _e: None):
        dirnames.sort()
        filenames.sort()
        for filename in filenames:
            files.append(Path(dirpath) / filename)
    return files



def _mtime(path: Path) -> float:
    try:
        return path.stat().st_mtime
    except OSError:
        return 0.0



def _glob_paths(root: Path, base: Path, pattern: str) -> tuple[list[Path], bool]:
    raw_matches = globlib.glob(str(base / pattern), recursive=True)
    unique: dict[str, Path] = {}
    for raw in raw_matches:
        try:
            resolved = ensure_within_root(root, Path(raw))
        except PermissionError:
            continue
        unique[str(resolved)] = resolved
    matches = sorted(unique.values(), key=_mtime, reverse=True)
    truncated = len(matches) > GLOB_LIMIT
    return matches[:GLOB_LIMIT], truncated



def _parse_grep_include(args: GrepParameters) -> str | None:
    if args.include:
        return args.include
    if args.glob:
        return args.glob
    return None



def _grep_with_rg(root: Path, base: Path, pattern: str, include_glob: str | None) -> list[dict[str, str | int | float]]:
    rg = shutil.which("rg")
    if not rg:
        raise FileNotFoundError("rg not found")

    command = [rg, "-nH", "--color", "never", "--field-match-separator=|", "--regexp", pattern]
    if include_glob:
        command.extend(["--glob", include_glob])
    command.append(str(base))

    completed = subprocess.run(command, capture_output=True, text=True)
    if completed.returncode == 1:
        return []
    if completed.returncode != 0:
        stderr = (completed.stderr or "").strip()
        raise RuntimeError(stderr or "ripgrep failed")

    matches: list[dict[str, str | int | float]] = []
    for raw_line in completed.stdout.splitlines():
        parts = raw_line.split("|", 2)
        if len(parts) != 3:
            continue
        file_path, line_number, line_text = parts
        try:
            resolved = ensure_within_root(root, Path(file_path))
        except PermissionError:
            continue
        matches.append(
            {
                "path": str(resolved),
                "line": int(line_number),
                "text": line_text,
                "mtime": _mtime(resolved),
            }
        )
    return matches



def _grep_with_python(root: Path, base: Path, pattern: str, include_glob: str | None) -> list[dict[str, str | int | float]]:
    regex = re.compile(pattern)
    matches: list[dict[str, str | int | float]] = []
    include = include_glob or "*"
    for file_path in _iter_files(base):
        if not fnmatch.fnmatch(file_path.name, include):
            continue
        try:
            resolved = ensure_within_root(root, file_path)
        except PermissionError:
            continue
        try:
            content = resolved.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        for line_number, line_text in enumerate(content.splitlines(), start=1):
            if regex.search(line_text):
                matches.append(
                    {
                        "path": str(resolved),
                        "line": line_number,
                        "text": line_text,
                        "mtime": _mtime(resolved),
                    }
                )
    return matches



def _render_grep_output(matches: list[dict[str, str | int | float]], *, truncated: bool) -> str:
    if not matches:
        return "No files found"

    output_lines = [f"Found {len(matches)} matches"]
    current_file = ""
    for match in matches:
        file_path = str(match["path"])
        if current_file != file_path:
            if current_file:
                output_lines.append("")
            current_file = file_path
            output_lines.append(f"{file_path}:")
        line_text = str(match["text"])
        if len(line_text) > MAX_LINE_LENGTH:
            line_text = line_text[:MAX_LINE_LENGTH] + "..."
        output_lines.append(f"  Line {match['line']}: {line_text}")

    if truncated:
        output_lines.append("")
        output_lines.append("(Results are truncated. Consider using a more specific path or pattern.)")
    return "\n".join(output_lines)



def _should_ignore(relative_path: str, name: str, patterns: list[str]) -> bool:
    normalized = relative_path.replace("\\", "/")
    for pattern in patterns:
        cleaned = pattern.replace("\\", "/")
        if cleaned.endswith("/"):
            prefix = cleaned.rstrip("/")
            if normalized == prefix or normalized.startswith(prefix + "/") or name == prefix:
                return True
        if fnmatch.fnmatch(name, cleaned) or fnmatch.fnmatch(normalized, cleaned):
            return True
    return False



def _tree_node(tree: dict[str, object], parts: tuple[str, ...]) -> dict[str, object]:
    node = tree
    for part in parts:
        dirs = node.setdefault("dirs", {})
        assert isinstance(dirs, dict)
        node = dirs.setdefault(part, {"dirs": {}, "files": []})
        assert isinstance(node, dict)
    return node



def _collect_ls_tree(base: Path, ignore_patterns: list[str]) -> tuple[dict[str, object], int, bool]:
    tree: dict[str, object] = {"dirs": {}, "files": []}
    file_count = 0
    truncated = False

    if base.is_file():
        node_files = tree.setdefault("files", [])
        assert isinstance(node_files, list)
        node_files.append(base.name)
        return tree, 1, False

    for dirpath, dirnames, filenames in os.walk(base, topdown=True, onerror=lambda _e: None):
        relative_dir = Path(dirpath).resolve().relative_to(base.resolve()) if Path(dirpath).resolve() != base.resolve() else Path(".")
        parts = tuple() if relative_dir == Path(".") else relative_dir.parts
        node = _tree_node(tree, parts)

        filtered_dirnames: list[str] = []
        for dirname in sorted(dirnames):
            relative_name = dirname if relative_dir == Path(".") else (relative_dir / dirname).as_posix()
            if _should_ignore(relative_name, dirname, ignore_patterns):
                continue
            filtered_dirnames.append(dirname)
            _tree_node(tree, parts + (dirname,))
        dirnames[:] = filtered_dirnames

        for filename in sorted(filenames):
            relative_name = filename if relative_dir == Path(".") else (relative_dir / filename).as_posix()
            if _should_ignore(relative_name, filename, ignore_patterns):
                continue
            files = node.setdefault("files", [])
            assert isinstance(files, list)
            files.append(filename)
            file_count += 1
            if file_count >= LS_LIMIT:
                truncated = True
                break
        if truncated:
            dirnames[:] = []
            break

    return tree, file_count, truncated



def _render_ls_tree(label: str, tree: dict[str, object], *, truncated: bool) -> str:
    lines: list[str] = [label]

    def render(node: dict[str, object], depth: int) -> None:
        dirs = node.get("dirs", {})
        files = node.get("files", [])
        assert isinstance(dirs, dict)
        assert isinstance(files, list)
        for dirname in sorted(dirs.keys()):
            lines.append(f"{'  ' * (depth + 1)}{dirname}/")
            child = dirs[dirname]
            assert isinstance(child, dict)
            render(child, depth + 1)
        for filename in sorted(files):
            lines.append(f"{'  ' * (depth + 1)}{filename}")

    render(tree, 0)
    if truncated:
        lines.append("")
        lines.append("(Results are truncated. Consider using a more specific path.)")
    return "\n".join(lines)


async def read_tool(args: ReadParameters, ctx: ToolContext) -> ToolOutput:
    if ctx.execution_mode == "opensandbox":
        from . import file_sandbox

        return await file_sandbox.read_tool(args, ctx)

    root = ctx.session_root.resolve()
    target = resolve_path_in_root(root, args.file_path)
    if not target.exists():
        suggestions = _suggest_paths(target)
        if suggestions:
            raise FileNotFoundError(f"File not found: {target}\n\nDid you mean one of these?\n" + "\n".join(suggestions))
        raise FileNotFoundError(f"File not found: {target}")
    if target.is_dir():
        raise IsADirectoryError(f"Path is a directory, not a file: {target}")

    text = _read_text(target)
    output, preview, truncated = _format_read_output_from_text(text, offset=args.offset, limit=args.limit)
    _remember_read(ctx, target, content=text, source_tool="read")
    return ToolOutput(title=_display_path(root, target), output=output, metadata={"preview": preview}, truncated=truncated)


async def write_tool(args: WriteParameters, ctx: ToolContext) -> ToolOutput:
    if ctx.execution_mode == "opensandbox":
        from . import file_sandbox

        return await file_sandbox.write_tool(args, ctx)

    root = ctx.session_root.resolve()
    target = resolve_path_in_root(root, args.file_path)
    existed = target.exists()
    _require_existing_file_was_read(ctx, target, action="writing")
    _write_text(target, args.content)
    _remember_read(ctx, target, content=args.content, source_tool="write")
    return ToolOutput(
        title=_display_path(root, target),
        output=f"Wrote {len(args.content)} chars to {target}",
        metadata={"file_path": str(target), "exists": existed},
    )


async def edit_tool(args: EditParameters, ctx: ToolContext) -> ToolOutput:
    if ctx.execution_mode == "opensandbox":
        from . import file_sandbox

        return await file_sandbox.edit_tool(args, ctx)

    root = ctx.session_root.resolve()
    target = resolve_path_in_root(root, args.file_path)
    _require_existing_file_was_read(ctx, target, action="editing")

    if args.old_string == "":
        _write_text(target, args.new_string)
        _remember_read(ctx, target, content=args.new_string, source_tool="edit")
        return ToolOutput(
            title=_display_path(root, target),
            output=f"Edited {target}",
            metadata={"file_path": str(target), "replace_all": args.replace_all},
        )

    if not target.exists():
        raise FileNotFoundError(f"File not found: {target}")
    if target.is_dir():
        raise IsADirectoryError(f"Path is a directory, not a file: {target}")

    text = target.read_text(encoding="utf-8")
    new_text = _replace_text(text, args.old_string, args.new_string, replace_all=args.replace_all)
    target.write_text(new_text, encoding="utf-8")
    _remember_read(ctx, target, content=new_text, source_tool="edit")
    return ToolOutput(
        title=_display_path(root, target),
        output=f"Edited {target}",
        metadata={"file_path": str(target), "replace_all": args.replace_all},
    )


async def glob_tool(args: GlobParameters, ctx: ToolContext) -> ToolOutput:
    if ctx.execution_mode == "opensandbox":
        from . import file_sandbox

        return await file_sandbox.glob_tool(args, ctx)

    root = ctx.session_root.resolve()
    base = resolve_optional_path(root, args.path)
    matches, truncated = _glob_paths(root, base, args.pattern)
    if not matches:
        output = "No files found"
    else:
        output_lines = [str(match) for match in matches]
        if truncated:
            output_lines.extend(["", "(Results are truncated. Consider using a more specific path or pattern.)"])
        output = "\n".join(output_lines)
    return ToolOutput(
        title=_display_path(root, base),
        output=output,
        metadata={"count": len(matches)},
        truncated=truncated,
    )


async def grep_tool(args: GrepParameters, ctx: ToolContext) -> ToolOutput:
    if ctx.execution_mode == "opensandbox":
        from . import file_sandbox

        return await file_sandbox.grep_tool(args, ctx)

    root = ctx.session_root.resolve()
    base = resolve_optional_path(root, args.path)
    include_glob = _parse_grep_include(args)
    try:
        matches = _grep_with_rg(root, base, args.pattern, include_glob)
    except FileNotFoundError:
        matches = _grep_with_python(root, base, args.pattern, include_glob)

    matches.sort(key=lambda item: (-float(item["mtime"]), str(item["path"]), int(item["line"])))
    truncated = len(matches) > GREP_LIMIT
    final_matches = matches[:GREP_LIMIT]
    output = _render_grep_output(final_matches, truncated=truncated)
    return ToolOutput(
        title=args.pattern,
        output=output,
        metadata={"count": len(final_matches), "include": include_glob or "*"},
        truncated=truncated,
    )


async def ls_tool(args: LsParameters, ctx: ToolContext) -> ToolOutput:
    if ctx.execution_mode == "opensandbox":
        from . import file_sandbox

        return await file_sandbox.ls_tool(args, ctx, default_ignore=DEFAULT_LS_IGNORE)

    root = ctx.session_root.resolve()
    base = resolve_optional_path(root, args.path)
    ignore_patterns = list(DEFAULT_LS_IGNORE)
    if args.ignore:
        ignore_patterns.extend(args.ignore)

    tree, count, truncated = _collect_ls_tree(base, ignore_patterns)
    output = _render_ls_tree(f"{base}/", tree, truncated=truncated)
    return ToolOutput(
        title=_display_path(root, base),
        output=output,
        metadata={"count": count, "ignore": ignore_patterns},
        truncated=truncated,
    )



def register(registry: ToolRegistry) -> None:
    workspace_read = ToolExecutionSchema.readonly(batch_group="workspace-read")
    file_read = ToolExecutionSchema.readonly(batch_group="workspace-read", mutates_session=True)
    file_write = ToolExecutionSchema.exclusive(
        batch_group="workspace-write",
        mutates_workspace=True,
        mutates_session=True,
        conflict_key_template="file:{file_path}",
    )
    registry.define_tool(
        tool_id="read",
        parameters=ReadParameters,
        description_md="read.md",
        group="file",
        dangerous=False,
        execution_scope="workspace",
        execution_schema=file_read,
    )(read_tool)
    registry.define_tool(
        tool_id="write",
        parameters=WriteParameters,
        description_md="write.md",
        group="file",
        dangerous=True,
        execution_scope="workspace",
        execution_schema=file_write,
    )(write_tool)
    registry.define_tool(
        tool_id="edit",
        parameters=EditParameters,
        description_md="edit.md",
        group="file",
        dangerous=True,
        execution_scope="workspace",
        execution_schema=file_write,
    )(edit_tool)
    registry.define_tool(
        tool_id="glob",
        parameters=GlobParameters,
        description_md="glob.md",
        group="file",
        dangerous=False,
        execution_scope="workspace",
        execution_schema=workspace_read,
    )(glob_tool)
    registry.define_tool(
        tool_id="grep",
        parameters=GrepParameters,
        description_md="grep.md",
        group="file",
        dangerous=False,
        execution_scope="workspace",
        execution_schema=workspace_read,
    )(grep_tool)
    registry.define_tool(
        tool_id="ls",
        parameters=LsParameters,
        description_md="ls.md",
        group="file",
        dangerous=False,
        execution_scope="workspace",
        execution_schema=workspace_read,
    )(ls_tool)


__all__ = ["register"]
