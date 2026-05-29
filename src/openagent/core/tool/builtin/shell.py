from __future__ import annotations

"""
Shell tool (bash).

Notes:
- This tool is dangerous and still goes through PermissionManager.
- workdir must remain inside the active workspace root.
- destructive delete commands are blocked before execution.
"""

import re
from dataclasses import dataclass, field
from pathlib import Path

from ...execution.runtime import LocalWorkspaceRuntime
from ..definition import ToolContext, ToolOutput
from ..registry import ToolRegistry

DEFAULT_TIMEOUT_MS = 120_000
FORBIDDEN_COMMAND_RE = re.compile(
    r"(?i)(?:^|[;&|]\s*)(rm|rmdir|del|erase|deltree|remove-item|shred|unlink)(?:\s|$)"
)


@dataclass
class BashParameters:
    command: str = field(metadata={"description": "Shell command to execute"})
    timeout: int = field(default=DEFAULT_TIMEOUT_MS, metadata={"description": "Timeout in milliseconds"})
    workdir: str | None = field(default=None, metadata={"description": "Working directory, defaults to workspace root"})
    description: str | None = field(default=None, metadata={"description": "Optional short description of the command purpose"})


def _blocked_command(command: str) -> str | None:
    match = FORBIDDEN_COMMAND_RE.search(command)
    if match:
        return match.group(1)
    return None


def _allow_destructive_commands(ctx: ToolContext) -> bool:
    if ctx.execution_mode in {"terminal_bench", "harbor"}:
        return True
    session = ctx.extra.get("session")
    metadata = getattr(session, "metadata", None)
    return isinstance(metadata, dict) and bool(metadata.get("allow_destructive_commands"))


def _workspace_runtime(ctx: ToolContext):
    runtime = ctx.workspace_runtime
    if runtime is not None:
        return runtime
    return LocalWorkspaceRuntime(ctx.session_root.resolve())


async def bash_tool(args: BashParameters, ctx: ToolContext) -> ToolOutput:
    if args.timeout < 0:
        raise ValueError(f"Invalid timeout value: {args.timeout}. Timeout must be a positive number.")

    blocked = _blocked_command(args.command)
    if blocked and not _allow_destructive_commands(ctx):
        raise ValueError(f"{blocked} command is disabled for security reasons")

    runtime = _workspace_runtime(ctx)
    command_result = await runtime.run_command(args.command, args.workdir, args.timeout)
    combined = ((command_result.stdout or "") + (command_result.stderr or "")).strip()
    output = combined or f"Command exited with return code {command_result.returncode}."

    if ctx.execution_mode in {"opensandbox", "terminal_bench", "harbor"} and hasattr(runtime, "display_path"):
        title = runtime.display_path(command_result.cwd)
    else:
        root = ctx.session_root.resolve()
        cwd = Path(command_result.cwd)
        try:
            title = str(cwd.relative_to(root))
        except Exception:  # noqa: BLE001
            title = str(cwd)

    metadata = {
        "returncode": command_result.returncode,
        "description": args.description or "",
        "workdir": command_result.cwd,
    }
    if ctx.execution_mode in {"opensandbox", "terminal_bench", "harbor"}:
        metadata.update(ctx.execution_metadata or {})

    return ToolOutput(
        title=title,
        output=output,
        metadata=metadata,
    )


def register(registry: ToolRegistry) -> None:
    registry.define_tool(
        tool_id="bash",
        parameters=BashParameters,
        description_md="bash.md",
        group="shell",
        dangerous=True,
        execution_scope="workspace",
    )(bash_tool)


__all__ = ["register"]
