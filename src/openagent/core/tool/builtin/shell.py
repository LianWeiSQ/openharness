from __future__ import annotations

"""
Shell tool (bash).

中文说明：
- 该工具属于高风险工具，默认需要 PermissionManager 放行
- workdir 必须限制在 session_root 内（避免越界执行）
- 删除类命令会被直接拦截，避免误删工作区内容
"""

import asyncio
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry
from ..utils import resolve_optional_path

DEFAULT_TIMEOUT_MS = 120_000
FORBIDDEN_COMMAND_RE = re.compile(
    r"(?i)(?:^|[;&|]\s*)(rm|rmdir|del|erase|deltree|remove-item|shred|unlink)(?:\s|$)"
)


@dataclass
class BashParameters:
    command: str = field(metadata={"description": "要执行的 shell 命令"})
    timeout: int = field(default=DEFAULT_TIMEOUT_MS, metadata={"description": "超时（毫秒）"})
    workdir: str | None = field(default=None, metadata={"description": "工作目录（默认 session_root）"})
    description: str | None = field(default=None, metadata={"description": "对命令目的的简短描述（可选）"})



def _blocked_command(command: str) -> str | None:
    match = FORBIDDEN_COMMAND_RE.search(command)
    if match:
        return match.group(1)
    return None


async def bash_tool(args: BashParameters, ctx: ToolContext) -> ToolOutput:
    if args.timeout < 0:
        raise ValueError(f"Invalid timeout value: {args.timeout}. Timeout must be a positive number.")

    blocked = _blocked_command(args.command)
    if blocked:
        raise ValueError(f"{blocked} command is disabled for security reasons")

    root = ctx.session_root.resolve()
    cwd = resolve_optional_path(root, args.workdir)

    def _run() -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            args.command,
            cwd=str(cwd),
            shell=True,
            capture_output=True,
            text=True,
            timeout=args.timeout / 1000.0,
        )

    completed = await asyncio.to_thread(_run)
    combined = ((completed.stdout or "") + (completed.stderr or "")).strip()
    output = combined or f"Command exited with return code {completed.returncode}."

    try:
        title = str(Path(cwd).relative_to(root))
    except Exception:  # noqa: BLE001
        title = str(cwd)

    return ToolOutput(
        title=title,
        output=output,
        metadata={
            "returncode": completed.returncode,
            "description": args.description or "",
            "workdir": str(cwd),
        },
    )



def register(registry: ToolRegistry) -> None:
    registry.define_tool(tool_id="bash", parameters=BashParameters, description_md="bash.md", group="shell", dangerous=True)(bash_tool)


__all__ = ["register"]
