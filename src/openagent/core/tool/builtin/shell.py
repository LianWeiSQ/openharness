from __future__ import annotations

"""
Shell tool (bash).

中文说明：
- 该工具属于高风险工具，默认需要 PermissionManager 放行
- workdir 必须限制在 session_root 内（避免越界执行）
"""

import asyncio
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry
from ..utils import resolve_optional_path


@dataclass
class BashParameters:
    command: str = field(metadata={"description": "要执行的 shell 命令"})
    timeout: int = field(default=120_000, metadata={"description": "超时（毫秒）"})
    workdir: str | None = field(default=None, metadata={"description": "工作目录（默认 session_root）"})


async def bash_tool(args: BashParameters, ctx: ToolContext) -> ToolOutput:
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
    out = (completed.stdout or "") + (completed.stderr or "")
    return ToolOutput(
        title=str(Path(cwd).relative_to(root)) if str(cwd).startswith(str(root)) else str(cwd),
        output=out.strip(),
        metadata={"returncode": completed.returncode},
    )


def register(registry: ToolRegistry) -> None:
    registry.define_tool(tool_id="bash", parameters=BashParameters, description_md="bash.md", group="shell", dangerous=True)(bash_tool)


__all__ = ["register"]

