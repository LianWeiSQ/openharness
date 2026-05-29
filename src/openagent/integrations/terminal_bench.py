from __future__ import annotations

import asyncio
import json
import os
import re
import shlex
import tempfile
import threading
import time
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any, Coroutine
from uuid import uuid4

from openagent.core.agent.universal import UniversalAgent
from openagent.core.execution.runtime import CommandResult
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.provider.base import LanguageModel
from openagent.core.provider.openai import OpenAIProvider
from openagent.core.session.session import Session
from openagent.core.types import AgentConfig, Model

try:  # pragma: no cover - covered by Terminal-Bench smoke runs when installed.
    from terminal_bench.agents import BaseAgent
    from terminal_bench.agents.base_agent import AgentResult
    from terminal_bench.agents.failure_mode import FailureMode
    from terminal_bench.terminal.models import TerminalCommand
    from terminal_bench.terminal.tmux_session import TmuxSession
except Exception:  # noqa: BLE001

    class FailureMode(Enum):
        NONE = "none"
        UNKNOWN_AGENT_ERROR = "unknown_agent_error"
        AGENT_TIMEOUT = "agent_timeout"
        CONTEXT_LENGTH_EXCEEDED = "context_length_exceeded"
        OUTPUT_LENGTH_EXCEEDED = "output_length_exceeded"

    @dataclass(slots=True)
    class AgentResult:
        total_input_tokens: int = 0
        total_output_tokens: int = 0
        failure_mode: FailureMode = FailureMode.NONE
        timestamped_markers: list[tuple[float, str]] | None = None

    class BaseAgent:
        def __init__(self, **kwargs: Any) -> None:
            del kwargs

    @dataclass(slots=True)
    class TerminalCommand:
        command: str
        min_timeout_sec: float = 0.0
        max_timeout_sec: float = 180.0
        block: bool = False
        append_enter: bool = True

    class TmuxSession:  # pragma: no cover - typing fallback only.
        pass


DEFAULT_MAX_STEPS = 80
DEFAULT_CONTEXT_WINDOW = 128_000
DEFAULT_MAX_OUTPUT = 4096
DEFAULT_WORKDIR = "/app"
EVENTS_FILENAME = "openagent-events.jsonl"
FINAL_ANSWER_FILENAME = "final-answer.txt"
ERROR_FILENAME = "openagent-error.txt"


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        value = int(raw)
    except ValueError:
        return default
    return value if value > 0 else default


def _failure_mode(name: str):
    return getattr(FailureMode, name, getattr(FailureMode, "UNKNOWN_AGENT_ERROR", FailureMode.NONE))


def _run_sync(coro: Coroutine[Any, Any, Any]) -> Any:
    try:
        running_loop = asyncio.get_running_loop()
    except RuntimeError:
        running_loop = None
    if running_loop is None or not running_loop.is_running():
        return asyncio.run(coro)

    result: dict[str, Any] = {}

    def _target() -> None:
        try:
            result["value"] = asyncio.run(coro)
        except BaseException as error:  # noqa: BLE001
            result["error"] = error

    thread = threading.Thread(target=_target, daemon=True)
    thread.start()
    thread.join()
    if "error" in result:
        raise result["error"]
    return result.get("value")


def _json_default(value: Any) -> Any:
    if isinstance(value, Enum):
        return value.value
    if isinstance(value, Path):
        return str(value)
    return str(value)


class TerminalBenchWorkspaceRuntime:
    mode = "terminal_bench"

    def __init__(self, session: TmuxSession, *, workspace_root: str = DEFAULT_WORKDIR) -> None:
        self.session = session
        self.workspace_root = workspace_root
        self._prime_output_tracking()

    @property
    def execution_metadata(self) -> dict[str, Any]:
        return {
            "execution_mode": self.mode,
            "workspace_root": self.workspace_root,
            "harness": "terminal_bench",
        }

    def display_path(self, path: str | Path) -> str:
        text = str(path)
        root = self.workspace_root.rstrip("/")
        if text == root:
            return "."
        prefix = root + "/"
        if text.startswith(prefix):
            return text[len(prefix) :]
        return text

    async def run_command(self, command: str, cwd: str | None, timeout_ms: int) -> CommandResult:
        timeout_sec = max(timeout_ms / 1000.0, 1.0)
        resolved_cwd = cwd or self.workspace_root
        marker = f"__OPENAGENT_TBENCH_EXIT_{uuid4().hex}__"
        wrapped = self._wrap_command(command=command, cwd=resolved_cwd, marker=marker)
        started = time.time()
        try:
            await asyncio.to_thread(
                self._send_command,
                TerminalCommand(
                    command=wrapped,
                    block=True,
                    max_timeout_sec=timeout_sec,
                    append_enter=True,
                ),
            )
            observation = await asyncio.to_thread(self._capture_observation)
            returncode, cleaned = self._extract_returncode(observation, marker)
            elapsed_ms = int((time.time() - started) * 1000)
            stdout = self._format_observation(cleaned, returncode=returncode, elapsed_ms=elapsed_ms)
            return CommandResult(returncode=returncode, stdout=stdout, stderr="", cwd=resolved_cwd)
        except TimeoutError as error:
            observation = await asyncio.to_thread(self._capture_observation)
            elapsed_ms = int((time.time() - started) * 1000)
            stdout = self._format_observation(observation, returncode=124, elapsed_ms=elapsed_ms)
            return CommandResult(returncode=124, stdout=stdout, stderr=str(error), cwd=resolved_cwd)

    def _wrap_command(self, *, command: str, cwd: str | None, marker: str) -> str:
        lines: list[str] = ["set +e"]
        if cwd:
            lines.append(f"cd {shlex.quote(cwd)}")
        lines.extend(
            [
                "(",
                command,
                ")",
                "status=$?",
                f"printf '\\n{marker}%s\\n' \"$status\"",
            ]
        )
        return f"bash -lc {shlex.quote(chr(10).join(lines))}"

    def _send_command(self, command: TerminalCommand) -> None:
        send_command = getattr(self.session, "send_command", None)
        if callable(send_command):
            send_command(command)
            return

        keys = [command.command, "Enter"] if command.append_enter else [command.command]
        self.session.send_keys(
            keys=keys,
            block=command.block,
            max_timeout_sec=command.max_timeout_sec,
            min_timeout_sec=command.min_timeout_sec,
        )

    def _capture_observation(self) -> str:
        get_incremental_output = getattr(self.session, "get_incremental_output", None)
        if callable(get_incremental_output):
            return str(get_incremental_output() or "")

        capture_pane = getattr(self.session, "capture_pane", None)
        if callable(capture_pane):
            return str(capture_pane(capture_entire=True) or "")
        return ""

    def _prime_output_tracking(self) -> None:
        get_incremental_output = getattr(self.session, "get_incremental_output", None)
        if not callable(get_incremental_output):
            return
        try:
            get_incremental_output()
        except Exception:  # noqa: BLE001
            return

    def _extract_returncode(self, observation: str, marker: str) -> tuple[int, str]:
        pattern = re.compile(rf"{re.escape(marker)}(?P<code>-?\d+)")
        matches = list(pattern.finditer(observation))
        if not matches:
            return 0, observation
        code = int(matches[-1].group("code"))
        cleaned = pattern.sub("", observation).strip()
        return code, cleaned

    def _format_observation(self, observation: str, *, returncode: int, elapsed_ms: int) -> str:
        body = observation.strip()
        suffix = f"[openagent terminal-bench] exit_code={returncode} duration_ms={elapsed_ms}"
        return f"{body}\n{suffix}" if body else suffix


class OpenAgentTerminalBenchAgent(BaseAgent):
    def __init__(
        self,
        *,
        language_model: LanguageModel | None = None,
        model: Model | None = None,
        max_steps: int | None = None,
        workspace_root: str | None = None,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self._language_model = language_model
        self._model = model
        self._max_steps = max_steps
        self._workspace_root = workspace_root or os.getenv("OPENAGENT_TBENCH_WORKDIR") or DEFAULT_WORKDIR

    @staticmethod
    def name() -> str:
        return "openagent"

    def perform_task(
        self,
        instruction: str,
        session: TmuxSession,
        logging_dir: Path | None = None,
    ) -> AgentResult:
        return _run_sync(self._perform_task_async(instruction=instruction, tmux_session=session, logging_dir=logging_dir))

    async def _perform_task_async(
        self,
        *,
        instruction: str,
        tmux_session: TmuxSession,
        logging_dir: Path | None,
    ) -> AgentResult:
        temp_dir: tempfile.TemporaryDirectory[str] | None = None
        if logging_dir is None:
            temp_dir = tempfile.TemporaryDirectory(prefix="openagent-tbench-")
            run_root = Path(temp_dir.name)
        else:
            run_root = Path(logging_dir)
            run_root.mkdir(parents=True, exist_ok=True)

        events_path = run_root / EVENTS_FILENAME if logging_dir is not None else None
        final_answer_path = run_root / FINAL_ANSWER_FILENAME if logging_dir is not None else None
        error_path = run_root / ERROR_FILENAME if logging_dir is not None else None
        session_root = run_root / "openagent-session"
        session_root.mkdir(parents=True, exist_ok=True)

        input_tokens = 0
        output_tokens = 0
        failure_mode = FailureMode.NONE
        final_chunks: list[str] = []

        try:
            model = self._model or self._model_from_env()
            language_model = self._language_model or await OpenAIProvider(
                wire_api=os.getenv("OPENAGENT_TBENCH_WIRE_API") or os.getenv("OPENAI_WIRE_API") or "chat"
            ).get_language_model(model)
            agent = UniversalAgent(
                config=AgentConfig(
                    name="terminal-bench",
                    mode="primary",
                    model=model,
                    tools=["bash"],
                    permission="FULL",
                    max_steps=self._max_steps or _env_int("OPENAGENT_MAX_STEPS", DEFAULT_MAX_STEPS),
                    temperature=float(os.getenv("OPENAGENT_TBENCH_TEMPERATURE", "0.2")),
                    options={
                        "observability": {
                            "enabled": True,
                            "keep_events": True,
                            "jsonl": logging_dir is not None,
                            "jsonl_dir": str(run_root / "traces"),
                        }
                    },
                ),
                model=language_model,
                system_prompt=self._system_prompt(),
            )
            openagent_session = Session(directory=session_root)
            openagent_session.metadata["allow_destructive_commands"] = True

            loop = AgentLoop(agent=agent, session=openagent_session, permission_manager=PermissionManager())
            loop.workspace_runtime = TerminalBenchWorkspaceRuntime(tmux_session, workspace_root=self._workspace_root)
            openagent_session.metadata["execution"] = {
                "mode": "terminal_bench",
                "workspace_root": self._workspace_root,
                "harness": "terminal_bench",
            }

            async for event in loop.run(instruction):
                self._append_event(events_path, event)
                if event.get("type") == "text-delta":
                    final_chunks.append(str(event.get("text") or ""))
                elif event.get("type") == "step-finish":
                    tokens = event.get("tokens") if isinstance(event.get("tokens"), dict) else {}
                    input_tokens += int(tokens.get("input") or 0)
                    output_tokens += int(tokens.get("output") or 0)
                elif event.get("type") == "error":
                    failure_mode = self._failure_mode_for_error(str(event.get("error") or ""))
                    self._write_text(error_path, str(event.get("error") or ""))

            self._write_text(final_answer_path, "".join(final_chunks).strip())
            return AgentResult(
                total_input_tokens=input_tokens,
                total_output_tokens=output_tokens,
                failure_mode=failure_mode,
            )
        except Exception as error:  # noqa: BLE001
            self._write_text(error_path, f"{type(error).__name__}: {error}")
            return AgentResult(
                total_input_tokens=input_tokens,
                total_output_tokens=output_tokens,
                failure_mode=self._failure_mode_for_error(str(error)),
            )
        finally:
            if temp_dir is not None:
                temp_dir.cleanup()

    def _model_from_env(self) -> Model:
        model_id = os.getenv("OPENAI_MODEL") or "gpt-4o-mini"
        return Model(
            id=model_id,
            provider_id="openai",
            name=f"OpenAI Compatible/{model_id}",
            context_window=_env_int("OPENAI_CONTEXT_WINDOW", DEFAULT_CONTEXT_WINDOW),
            max_output=_env_int("OPENAI_MAX_OUTPUT", DEFAULT_MAX_OUTPUT),
        )

    def _system_prompt(self) -> str:
        return (
            "You are OpenAgent running inside Terminal-Bench. Complete the task by using only the bash tool.\n"
            f"The bash tool executes commands in the benchmark tmux session. The default workspace is {self._workspace_root}.\n"
            "Directory changes do not persist between tool calls, so use absolute paths or combine commands with `cd <dir> && ...`.\n"
            "Inspect the environment, modify files with shell commands when needed, run validation commands, and iterate from failures.\n"
            "Do not ask the user questions. When the task is complete, provide a concise final answer."
        )

    def _append_event(self, path: Path | None, event: dict[str, Any]) -> None:
        if path is None:
            return
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(event, ensure_ascii=False, default=_json_default) + "\n")

    def _write_text(self, path: Path | None, content: str) -> None:
        if path is None:
            return
        path.write_text(content, encoding="utf-8")

    def _failure_mode_for_error(self, message: str):
        lowered = message.lower()
        if "timeout" in lowered:
            return _failure_mode("AGENT_TIMEOUT")
        if "context" in lowered and "length" in lowered:
            return _failure_mode("CONTEXT_LENGTH_EXCEEDED")
        if "output" in lowered and "length" in lowered:
            return _failure_mode("OUTPUT_LENGTH_EXCEEDED")
        return _failure_mode("UNKNOWN_AGENT_ERROR")


__all__ = [
    "OpenAgentTerminalBenchAgent",
    "TerminalBenchWorkspaceRuntime",
]
