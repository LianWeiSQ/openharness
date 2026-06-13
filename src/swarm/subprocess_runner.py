from __future__ import annotations

"""Subprocess runner for external CLI agents."""

import asyncio
import json
import os
import shlex
from collections.abc import AsyncIterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .config import SwarmConfig
from .payload import payload_for_runner
from .protocol import AgentDescriptor, AgentEvent, AgentResult, AgentSpec, RunContext
from .registry import RunnerRegistry
from .results import normalize_result_payload


@dataclass(frozen=True, slots=True)
class SubprocessCommand:
    argv: list[str]
    cwd: str | None = None
    env: dict[str, str] | None = None
    timeout_seconds: float | None = None


class SubprocessRunHandle:
    def __init__(self, *, events: list[AgentEvent], result: AgentResult) -> None:
        self._events = events
        self._result = result
        self._cancelled = False

    async def events(self) -> AsyncIterator[AgentEvent]:
        for event in self._events:
            yield event

    async def result(self) -> AgentResult:
        if self._cancelled:
            return AgentResult(status="cancelled", summary="Subprocess runner was cancelled.")
        return self._result

    async def cancel(self) -> None:
        self._cancelled = True


class SubprocessRunner:
    def __init__(self, *, descriptor: AgentDescriptor, command: SubprocessCommand) -> None:
        if not command.argv:
            raise ValueError("subprocess command argv is required")
        self._descriptor = descriptor
        self.command = command

    @property
    def descriptor(self) -> AgentDescriptor:
        return self._descriptor

    async def start(self, spec: AgentSpec, ctx: RunContext) -> SubprocessRunHandle:
        spec.validate()
        started = AgentEvent(
            type="runner.started",
            run_id=ctx.run_id,
            runner_id=self.descriptor.id,
            message=f"Started {self.descriptor.id}",
            metadata={"role": spec.role, "kind": self.descriptor.kind, "transport": "subprocess"},
        )
        result = await self._run_process(spec=spec, ctx=ctx)
        finished = AgentEvent(
            type="runner.finished",
            run_id=ctx.run_id,
            runner_id=self.descriptor.id,
            message=result.summary,
            metadata={"status": result.status, "confidence": result.confidence, "transport": "subprocess"},
        )
        return SubprocessRunHandle(events=[started, finished], result=result)

    async def _run_process(self, *, spec: AgentSpec, ctx: RunContext) -> AgentResult:
        payload = payload_for_runner(spec=spec, ctx=ctx, descriptor=self.descriptor)
        timeout = spec.limits.timeout_seconds or self.command.timeout_seconds
        env = os.environ.copy()
        env.update(self.command.env or {})
        try:
            proc = await asyncio.create_subprocess_exec(
                *self.command.argv,
                cwd=self.command.cwd,
                env=env,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
        except Exception as error:  # noqa: BLE001
            return AgentResult(
                status="failed",
                summary=str(error),
                metadata={"error_kind": "subprocess_start_error", "runner_id": self.descriptor.id},
            )

        try:
            stdout, stderr = await asyncio.wait_for(
                proc.communicate(json.dumps(payload, ensure_ascii=False).encode("utf-8")),
                timeout=timeout,
            )
        except asyncio.TimeoutError:
            proc.kill()
            await proc.wait()
            return AgentResult(
                status="failed",
                summary=f"Subprocess runner timed out after {timeout} seconds.",
                metadata={"error_kind": "subprocess_timeout", "runner_id": self.descriptor.id},
            )

        stdout_text = stdout.decode("utf-8", errors="replace").strip()
        stderr_text = stderr.decode("utf-8", errors="replace").strip()
        if proc.returncode != 0:
            return AgentResult(
                status="failed",
                summary=stderr_text or stdout_text or f"Subprocess exited with code {proc.returncode}.",
                metadata={
                    "error_kind": "subprocess_exit_error",
                    "runner_id": self.descriptor.id,
                    "returncode": proc.returncode,
                    "stderr": stderr_text,
                },
            )
        if not stdout_text:
            return AgentResult(
                status="completed",
                summary="Subprocess completed without stdout.",
                metadata={"runner_id": self.descriptor.id, "returncode": proc.returncode},
            )
        try:
            decoded = json.loads(stdout_text)
        except json.JSONDecodeError:
            return AgentResult(
                status="completed",
                summary=stdout_text,
                metadata={"runner_id": self.descriptor.id, "returncode": proc.returncode, "stdout_format": "text"},
            )
        result = normalize_result_payload(decoded if isinstance(decoded, dict) else str(decoded))
        return AgentResult(
            status=result.status,
            summary=result.summary,
            evidence=list(result.evidence),
            open_questions=list(result.open_questions),
            confidence=result.confidence,
            artifacts=list(result.artifacts),
            usage=result.usage,
            metadata={
                **dict(result.metadata or {}),
                "runner_id": self.descriptor.id,
                "returncode": proc.returncode,
                "stdout_format": "json",
            },
        )


def build_subprocess_registry(config: SwarmConfig) -> RunnerRegistry:
    registry = RunnerRegistry()
    for runner_config in config.runners:
        if runner_config.kind != "subprocess":
            continue
        registry.register(
            SubprocessRunner(
                descriptor=runner_config.to_descriptor(),
                command=_command_from_metadata(runner_config.metadata),
            )
        )
    return registry


def _command_from_metadata(metadata: dict[str, Any]) -> SubprocessCommand:
    raw_command = metadata.get("command") or metadata.get("argv")
    if raw_command is None:
        raise ValueError("subprocess runner metadata.command is required")
    if isinstance(raw_command, str):
        argv = shlex.split(raw_command)
    elif isinstance(raw_command, list):
        argv = [str(item) for item in raw_command]
    else:
        raise ValueError("subprocess command must be a string or list")
    cwd = metadata.get("cwd")
    env_raw = metadata.get("env") or {}
    if not isinstance(env_raw, dict):
        raise ValueError("subprocess env must be a mapping")
    timeout = metadata.get("timeout_seconds")
    return SubprocessCommand(
        argv=argv,
        cwd=str(Path(str(cwd)).resolve()) if cwd else None,
        env={str(key): str(value) for key, value in env_raw.items()},
        timeout_seconds=float(timeout) if timeout is not None else None,
    )

__all__ = ["SubprocessCommand", "SubprocessRunHandle", "SubprocessRunner", "build_subprocess_registry"]
