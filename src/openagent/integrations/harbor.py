from __future__ import annotations

import json
import math
import os
import tempfile
import time
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any

from openagent.core.agent.universal import UniversalAgent
from openagent.core.execution.runtime import CommandResult
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.provider.base import LanguageModel
from openagent.core.provider.openai import OpenAIProvider
from openagent.core.runtime_warnings import format_runtime_warning_event, runtime_warning_options_from_env
from openagent.core.session.session import Session
from openagent.core.types import AgentConfig, Model

try:  # pragma: no cover - exercised by Harbor smoke runs when installed.
    from harbor.agents.base import BaseAgent as HarborBaseAgent
    from harbor.environments.base import BaseEnvironment, ExecResult
    from harbor.models.agent.context import AgentContext
except Exception:  # noqa: BLE001

    class HarborBaseAgent:
        def __init__(
            self,
            logs_dir: Path | str | None = None,
            model_name: str | None = None,
            logger: Any | None = None,
            mcp_servers: list[Any] | None = None,
            skills_dir: str | None = None,
            **kwargs: Any,
        ) -> None:
            del logger, mcp_servers, kwargs
            self.logs_dir = Path(logs_dir or tempfile.mkdtemp(prefix="openagent-harbor-"))
            self.model_name = model_name
            self.skills_dir = skills_dir

    @dataclass(slots=True)
    class ExecResult:
        stdout: str | None = None
        stderr: str | None = None
        return_code: int = 0

    class BaseEnvironment:  # pragma: no cover - typing fallback only.
        async def exec(
            self,
            command: str,
            cwd: str | None = None,
            env: dict[str, str] | None = None,
            timeout_sec: int | None = None,
            user: str | int | None = None,
        ) -> ExecResult:
            del command, cwd, env, timeout_sec, user
            return ExecResult()

    @dataclass(slots=True)
    class AgentContext:
        n_input_tokens: int | None = None
        n_cache_tokens: int | None = None
        n_output_tokens: int | None = None
        cost_usd: float | None = None
        rollout_details: list[Any] | None = None
        metadata: dict[str, Any] | None = None


DEFAULT_MAX_STEPS = 80
DEFAULT_CONTEXT_WINDOW = 128_000
DEFAULT_MAX_OUTPUT = 4096
DEFAULT_WORKDIR = "/app"
EVENTS_FILENAME = "openagent-events.jsonl"
FINAL_ANSWER_FILENAME = "final-answer.txt"
ERROR_FILENAME = "openagent-error.txt"
RUNTIME_WARNINGS_FILENAME = "runtime-warnings.txt"


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        value = int(raw)
    except ValueError:
        return default
    return value if value > 0 else default


def _json_default(value: Any) -> Any:
    if isinstance(value, Enum):
        return value.value
    if isinstance(value, Path):
        return str(value)
    return str(value)


class HarborWorkspaceRuntime:
    mode = "harbor"

    def __init__(self, environment: BaseEnvironment, *, workspace_root: str = DEFAULT_WORKDIR) -> None:
        self.environment = environment
        self.workspace_root = workspace_root

    @property
    def execution_metadata(self) -> dict[str, Any]:
        return {
            "execution_mode": self.mode,
            "workspace_root": self.workspace_root,
            "harness": "harbor",
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
        resolved_cwd = cwd or self.workspace_root
        timeout_sec = max(1, math.ceil(timeout_ms / 1000.0))
        started = time.time()
        try:
            result = await self.environment.exec(command, cwd=resolved_cwd, timeout_sec=timeout_sec)
        except TimeoutError as error:
            elapsed_ms = int((time.time() - started) * 1000)
            return CommandResult(
                returncode=124,
                stdout=f"[openagent harbor] exit_code=124 duration_ms={elapsed_ms}",
                stderr=str(error),
                cwd=resolved_cwd,
            )

        elapsed_ms = int((time.time() - started) * 1000)
        returncode = int(getattr(result, "return_code", 0) or 0)
        stdout = str(getattr(result, "stdout", "") or "")
        stderr = str(getattr(result, "stderr", "") or "")
        suffix = f"[openagent harbor] exit_code={returncode} duration_ms={elapsed_ms}"
        formatted_stdout = f"{stdout.rstrip()}\n{suffix}" if stdout.strip() else suffix
        return CommandResult(returncode=returncode, stdout=formatted_stdout, stderr=stderr, cwd=resolved_cwd)


class OpenAgentHarborAgent(HarborBaseAgent):
    SUPPORTS_ATIF = False
    SUPPORTS_WINDOWS = False

    def __init__(
        self,
        logs_dir: Path | str | None = None,
        model_name: str | None = None,
        *,
        language_model: LanguageModel | None = None,
        model: Model | None = None,
        max_steps: int | None = None,
        workspace_root: str | None = None,
        **kwargs: Any,
    ) -> None:
        super().__init__(logs_dir=Path(logs_dir or tempfile.mkdtemp(prefix="openagent-harbor-")), model_name=model_name, **kwargs)
        self._language_model = language_model
        self._model = model
        self._max_steps = max_steps
        self._workspace_root = workspace_root or os.getenv("OPENAGENT_HARBOR_WORKDIR") or DEFAULT_WORKDIR

    @staticmethod
    def name() -> str:
        return "openagent"

    def version(self) -> str | None:
        return "harbor-adapter-local"

    async def setup(self, environment: BaseEnvironment) -> None:
        del environment

    async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
        run_root = Path(self.logs_dir)
        run_root.mkdir(parents=True, exist_ok=True)

        events_path = run_root / EVENTS_FILENAME
        final_answer_path = run_root / FINAL_ANSWER_FILENAME
        error_path = run_root / ERROR_FILENAME
        runtime_warnings_path = run_root / RUNTIME_WARNINGS_FILENAME
        session_root = run_root / "openagent-session"
        session_root.mkdir(parents=True, exist_ok=True)

        input_tokens = 0
        output_tokens = 0
        final_chunks: list[str] = []
        metadata: dict[str, Any] = {
            "harness": "harbor",
            "execution_mode": "harbor",
            "workspace_root": self._workspace_root,
            "events_path": str(events_path),
            "final_answer_path": str(final_answer_path),
        }

        try:
            model = self._model or self._model_from_env()
            metadata["model"] = model.id
            language_model = self._language_model or await OpenAIProvider(
                wire_api=os.getenv("OPENAGENT_HARBOR_WIRE_API") or os.getenv("OPENAI_WIRE_API") or "responses"
            ).get_language_model(model)
            agent = UniversalAgent(
                config=AgentConfig(
                    name="terminal-bench-2",
                    mode="primary",
                    model=model,
                    tools=["bash"],
                    permission="FULL",
                    max_steps=self._max_steps or _env_int("OPENAGENT_MAX_STEPS", DEFAULT_MAX_STEPS),
                    temperature=float(os.getenv("OPENAGENT_HARBOR_TEMPERATURE", "0.2")),
                    options={
                        "observability": {
                            "enabled": True,
                            "keep_events": True,
                            "jsonl": True,
                            "jsonl_dir": str(run_root / "traces"),
                        },
                        "runtime_warnings": runtime_warning_options_from_env(prefixes=("OPENAGENT_HARBOR", "OPENAGENT")),
                    },
                ),
                model=language_model,
                system_prompt=self._system_prompt(),
            )
            openagent_session = Session(directory=session_root)
            openagent_session.metadata["allow_destructive_commands"] = True

            loop = AgentLoop(agent=agent, session=openagent_session, permission_manager=PermissionManager())
            loop.workspace_runtime = HarborWorkspaceRuntime(environment, workspace_root=self._workspace_root)
            openagent_session.metadata["execution"] = {
                "mode": "harbor",
                "workspace_root": self._workspace_root,
                "harness": "harbor",
            }

            async for event in loop.run(instruction):
                self._append_event(events_path, event)
                warning_line = format_runtime_warning_event(event)
                if warning_line:
                    self._append_line(runtime_warnings_path, warning_line)
                    metadata.setdefault("runtime_warnings", []).append(
                        {
                            "code": event.get("code"),
                            "severity": event.get("severity"),
                            "message": event.get("message"),
                            "display": event.get("display"),
                        }
                    )
                if event.get("type") == "text-delta":
                    final_chunks.append(str(event.get("text") or ""))
                elif event.get("type") == "step-finish":
                    tokens = event.get("tokens") if isinstance(event.get("tokens"), dict) else {}
                    input_tokens += int(tokens.get("input") or 0)
                    output_tokens += int(tokens.get("output") or 0)
                elif event.get("type") == "error":
                    metadata["failure_mode"] = "agent_error"
                    self._write_text(error_path, str(event.get("error") or ""))

            final_answer = "".join(final_chunks).strip()
            self._write_text(final_answer_path, final_answer)
            metadata.setdefault("failure_mode", "none")
            metadata["final_answer"] = final_answer
        except Exception as error:  # noqa: BLE001
            metadata["failure_mode"] = "unknown_agent_error"
            metadata["error"] = f"{type(error).__name__}: {error}"
            self._write_text(error_path, metadata["error"])
        finally:
            context.n_input_tokens = input_tokens
            context.n_output_tokens = output_tokens
            context.cost_usd = 0.0
            context.metadata = metadata

    def _model_from_env(self) -> Model:
        model_id = self._normalized_model_name(self.model_name) or os.getenv("OPENAI_MODEL") or "gpt-4o-mini"
        return Model(
            id=model_id,
            provider_id="openai",
            name=f"OpenAI Compatible/{model_id}",
            context_window=_env_int("OPENAI_CONTEXT_WINDOW", DEFAULT_CONTEXT_WINDOW),
            max_output=_env_int("OPENAI_MAX_OUTPUT", DEFAULT_MAX_OUTPUT),
        )

    def _normalized_model_name(self, value: str | None) -> str | None:
        raw = (value or "").strip()
        if not raw:
            return None
        if "/" not in raw:
            return raw
        provider, model_name = raw.split("/", maxsplit=1)
        return model_name if provider.lower() in {"openai", "openai-compatible"} else raw

    def _system_prompt(self) -> str:
        return (
            "You are OpenAgent running inside Terminal-Bench 2.0 through Harbor. Complete the task by using only the bash tool.\n"
            f"The bash tool executes commands in the benchmark environment. The default workspace is {self._workspace_root}.\n"
            "Each tool call can pass an explicit workdir; otherwise it runs in the default workspace.\n"
            "Inspect the environment, modify files with shell commands when needed, run validation commands, and iterate from failures.\n"
            "Do not ask the user questions. When the task is complete, provide a concise final answer."
        )

    def _append_event(self, path: Path, event: dict[str, Any]) -> None:
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(event, ensure_ascii=False, default=_json_default) + "\n")

    def _write_text(self, path: Path, content: str) -> None:
        path.write_text(content, encoding="utf-8")

    def _append_line(self, path: Path, line: str) -> None:
        with path.open("a", encoding="utf-8") as handle:
            handle.write(line.rstrip() + "\n")


__all__ = [
    "HarborWorkspaceRuntime",
    "OpenAgentHarborAgent",
    "RUNTIME_WARNINGS_FILENAME",
]
