from __future__ import annotations

import os
import subprocess
from pathlib import Path
from typing import Any

from ..toolkit import ToolkitAdapter


def _ensure_within_root(root: Path, target: Path) -> Path:
    root_resolved = root.resolve()
    target_resolved = target.resolve()
    if root_resolved not in target_resolved.parents and root_resolved != target_resolved:
        raise PermissionError(f"Path escapes session root: {target}")
    return target_resolved


def register_shell_tools(toolkit: ToolkitAdapter) -> None:
    async def bash_tool(params: dict[str, Any], ctx: dict[str, Any]) -> str:
        command = str(params["command"])
        timeout = int(params.get("timeout", 120_000))
        workdir = str(params.get("workdir") or ctx.get("session_root") or os.getcwd())
        root = Path(str(ctx.get("session_root") or os.getcwd()))
        cwd = _ensure_within_root(root, Path(workdir))
        completed = subprocess.run(
            command,
            cwd=str(cwd),
            shell=True,
            capture_output=True,
            text=True,
            timeout=timeout / 1000.0,
        )
        out = (completed.stdout or "") + (completed.stderr or "")
        return out.strip()

    toolkit.register_tool(
        "bash",
        bash_tool,
        description="Execute a shell command inside the session root.",
        schema={
            "type": "object",
            "properties": {"command": {"type": "string"}, "timeout": {"type": "integer"}, "workdir": {"type": "string"}},
            "required": ["command"],
        },
        group="shell",
        dangerous=True,
    )

