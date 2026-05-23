from __future__ import annotations

import posixpath
import re
from pathlib import PurePosixPath
from typing import Any

from ...execution.runtime import OpenSandboxWorkspaceRuntime, WorkspaceEntry
from ...file_context import record_virtual_file_read
from ...session.session import Session
from ..definition import ToolContext, ToolOutput

MAX_LINE_LENGTH = 2000
MAX_READ_BYTES = 50 * 1024
GLOB_LIMIT = 100
GREP_LIMIT = 100
LS_LIMIT = 100


def _runtime(ctx: ToolContext) -> OpenSandboxWorkspaceRuntime:
    runtime = ctx.workspace_runtime
    if not isinstance(runtime, OpenSandboxWorkspaceRuntime):
        raise RuntimeError("Missing OpenSandbox workspace runtime in tool context.")
    return runtime


def _session_from_ctx(ctx: ToolContext) -> Session | None:
    session = ctx.extra.get("session")
    if isinstance(session, Session):
        return session
    return None


def _file_key(ctx: ToolContext, path: str) -> str:
    sandbox_id = str((ctx.execution_metadata or {}).get("sandbox_id") or "").strip()
    if sandbox_id:
        return f"opensandbox://{sandbox_id}{path}"
    return path


def _remember_read(ctx: ToolContext, path: str, *, content: str | bytes | None = None, source_tool: str = "read") -> None:
    session = _session_from_ctx(ctx)
    if session is not None:
        key = _file_key(ctx, path)
        session.remember_file_read(key)
        if content is not None:
            record_virtual_file_read(
                session.metadata,
                absolute_path=key,
                display_path=path,
                content=content,
                source_tool=source_tool,
            )


async def _require_existing_file_was_read(ctx: ToolContext, runtime: OpenSandboxWorkspaceRuntime, path: str, *, action: str) -> None:
    session = _session_from_ctx(ctx)
    if session is None:
        return
    if not await runtime.exists(path):
        return
    if not session.has_read_file(_file_key(ctx, path)):
        raise ValueError(f"Must read existing file before {action} it: {path}")


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
    output_lines.append("</file>")
    return "\n".join(output_lines), "\n".join(preview_lines), truncated


def _replace_text(content: str, old: str, new: str, *, replace_all: bool) -> str:
    if old == new:
        raise ValueError("old_string and new_string must be different")
    occurrences = content.count(old)
    if occurrences == 0:
        raise ValueError("old_string not found in content")
    if occurrences > 1 and not replace_all:
        raise ValueError(
            "old_string found multiple times; pass replace_all=True to replace every match"
        )
    return content.replace(old, new) if replace_all else content.replace(old, new, 1)


async def read_tool(args: Any, ctx: ToolContext) -> ToolOutput:
    runtime = _runtime(ctx)
    target = runtime.resolve_file_path(args.file_path)
    if not await runtime.exists(target):
        raise FileNotFoundError(f"File not found: {target}")
    if await runtime.is_dir(target):
        raise IsADirectoryError(f"Path is a directory, not a file: {target}")

    text = await runtime.read_text(target)
    if "\x00" in text:
        raise ValueError(f"Cannot read binary file: {target}")
    output, preview, truncated = _format_read_output_from_text(text, offset=args.offset, limit=args.limit)
    _remember_read(ctx, target, content=text, source_tool="read")
    return ToolOutput(title=runtime.display_path(target), output=output, metadata={"preview": preview}, truncated=truncated)


async def write_tool(args: Any, ctx: ToolContext) -> ToolOutput:
    runtime = _runtime(ctx)
    target = runtime.resolve_file_path(args.file_path)
    existed = await runtime.exists(target)
    await _require_existing_file_was_read(ctx, runtime, target, action="writing")
    await runtime.write_text(target, args.content)
    _remember_read(ctx, target, content=args.content, source_tool="write")
    metadata = {"file_path": target, "exists": existed}
    metadata.update(ctx.execution_metadata or {})
    return ToolOutput(
        title=runtime.display_path(target),
        output=f"Wrote {len(args.content)} chars to {target}",
        metadata=metadata,
    )


async def edit_tool(args: Any, ctx: ToolContext) -> ToolOutput:
    runtime = _runtime(ctx)
    target = runtime.resolve_file_path(args.file_path)
    await _require_existing_file_was_read(ctx, runtime, target, action="editing")
    if not await runtime.exists(target):
        raise FileNotFoundError(f"File not found: {target}")
    if await runtime.is_dir(target):
        raise IsADirectoryError(f"Path is a directory, not a file: {target}")

    if args.old_string == "":
        await runtime.write_text(target, args.new_string)
        new_text = args.new_string
    else:
        text = await runtime.read_text(target)
        new_text = _replace_text(text, args.old_string, args.new_string, replace_all=args.replace_all)
        await runtime.write_text(target, new_text)
    _remember_read(ctx, target, content=new_text, source_tool="edit")
    metadata = {"file_path": target, "replace_all": args.replace_all}
    metadata.update(ctx.execution_metadata or {})
    return ToolOutput(
        title=runtime.display_path(target),
        output=f"Edited {target}",
        metadata=metadata,
    )


async def glob_tool(args: Any, ctx: ToolContext) -> ToolOutput:
    runtime = _runtime(ctx)
    base = runtime.resolve_path(args.path, default_to_root=True)
    matches = await runtime.glob(base, args.pattern)
    truncated = len(matches) > GLOB_LIMIT
    final_matches = matches[:GLOB_LIMIT]
    if not final_matches:
        output = "No files found"
    else:
        output_lines = list(final_matches)
        if truncated:
            output_lines.extend(["", "(Results are truncated. Consider using a more specific path or pattern.)"])
        output = "\n".join(output_lines)
    metadata = {"count": len(final_matches)}
    metadata.update(ctx.execution_metadata or {})
    return ToolOutput(
        title=runtime.display_path(base),
        output=output,
        metadata=metadata,
        truncated=truncated,
    )


async def grep_tool(args: Any, ctx: ToolContext) -> ToolOutput:
    runtime = _runtime(ctx)
    base = runtime.resolve_path(args.path, default_to_root=True)
    include_glob = args.include or args.glob
    matches = await runtime.grep(base, args.pattern, include_glob)
    matches.sort(key=lambda item: (-float(item["mtime"]), str(item["path"]), int(item["line"])))
    truncated = len(matches) > GREP_LIMIT
    final_matches = matches[:GREP_LIMIT]
    output = _render_grep_output(final_matches, truncated=truncated)
    metadata = {"count": len(final_matches), "include": include_glob or "*"}
    metadata.update(ctx.execution_metadata or {})
    return ToolOutput(
        title=args.pattern,
        output=output,
        metadata=metadata,
        truncated=truncated,
    )


async def ls_tool(args: Any, ctx: ToolContext, *, default_ignore: list[str]) -> ToolOutput:
    runtime = _runtime(ctx)
    base = runtime.resolve_path(args.path, default_to_root=True)
    ignore_patterns = list(default_ignore)
    if args.ignore:
        ignore_patterns.extend(args.ignore)
    entries = await runtime.ls(base, ignore_patterns)
    tree, count, truncated = _collect_ls_tree(entries, base)
    output = _render_ls_tree(f"{base}/", tree, truncated=truncated)
    metadata = {"count": count, "ignore": ignore_patterns}
    metadata.update(ctx.execution_metadata or {})
    return ToolOutput(
        title=runtime.display_path(base),
        output=output,
        metadata=metadata,
        truncated=truncated,
    )


def _render_grep_output(matches: list[dict[str, str | int | float]], *, truncated: bool) -> str:
    if not matches:
        return "No files found"

    output_lines = ["<grep>"]
    for match in matches:
        output_lines.append(f"{match['path']}:{match['line']}:{match['text']}")
    output_lines.append("")
    output_lines.append(f"Found {len(matches)} matches")
    if truncated:
        output_lines.append("(Results are truncated. Consider using a more specific path or pattern.)")
    output_lines.append("</grep>")
    return "\n".join(output_lines)


def _collect_ls_tree(entries: list[WorkspaceEntry], base: str) -> tuple[dict[str, object], int, bool]:
    tree: dict[str, object] = {"dirs": {}, "files": []}
    count = 0
    seen_dirs: set[str] = set()
    seen_files: set[str] = set()
    truncated = len(entries) > LS_LIMIT
    for entry in entries[:LS_LIMIT]:
        rel = posixpath.relpath(entry.path, start=base)
        if rel in {".", ""}:
            continue
        parts = tuple(part for part in PurePosixPath(rel).parts if part and part != ".")
        if not parts:
            continue
        parent = _tree_node(tree, parts[:-1])
        if entry.is_dir:
            if rel in seen_dirs:
                continue
            seen_dirs.add(rel)
            dirs = parent["dirs"]
            assert isinstance(dirs, dict)
            dirs.setdefault(parts[-1], {"dirs": {}, "files": []})
            count += 1
            continue
        key = "/".join(parts)
        if key in seen_files:
            continue
        seen_files.add(key)
        files = parent["files"]
        assert isinstance(files, list)
        files.append(parts[-1])
        count += 1
    return tree, count, truncated


def _tree_node(tree: dict[str, object], parts: tuple[str, ...]) -> dict[str, object]:
    node = tree
    for part in parts:
        dirs = node["dirs"]
        assert isinstance(dirs, dict)
        node = dirs.setdefault(part, {"dirs": {}, "files": []})
        assert isinstance(node, dict)
    return node


def _render_ls_tree(label: str, tree: dict[str, object], *, truncated: bool) -> str:
    lines: list[str] = [label]

    def walk(node: dict[str, object], prefix: str) -> None:
        dirs = node["dirs"]
        files = node["files"]
        assert isinstance(dirs, dict)
        assert isinstance(files, list)

        for dirname in sorted(dirs.keys()):
            lines.append(f"{prefix}{dirname}/")
            child = dirs[dirname]
            assert isinstance(child, dict)
            walk(child, prefix + "  ")
        for filename in sorted(files):
            lines.append(f"{prefix}{filename}")

    walk(tree, "")
    if truncated:
        lines.append("")
        lines.append("(Results are truncated. Consider using a more specific path.)")
    return "\n".join(lines)
