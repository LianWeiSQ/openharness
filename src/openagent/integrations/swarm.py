from __future__ import annotations

"""OpenAgent adapter for the agent-agnostic swarm kernel."""

import asyncio
from contextlib import suppress
import json
import time
from collections.abc import AsyncIterator, Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal

from swarm import AgentDescriptor, AgentEvent, AgentResult, AgentSpec, RunContext, Usage
from swarm.config import RunnerConfig, SwarmConfig
from swarm.registry import RunnerRegistry

from ..core.agent.universal import UniversalAgent
from ..core.loop.processor import AgentLoop
from ..core.permission.manager import PermissionManager
from ..core.provider.base import LanguageModel
from ..core.session.session import Session
from ..core.tool.toolkit import ToolkitAdapter
from ..core.types import AgentConfig, Model, PermissionRulesetName

OpenAgentTools = list[str] | Literal["all", "readonly"]
ToolkitFactory = Callable[[], ToolkitAdapter]
PermissionManagerFactory = Callable[[], PermissionManager]


@dataclass(slots=True)
class OpenAgentRunHandle:
    _task: asyncio.Task[AgentResult]
    _events: list[AgentEvent] = field(default_factory=list)

    async def events(self) -> AsyncIterator[AgentEvent]:
        await self._wait()
        for event in self._events:
            yield event

    async def result(self) -> AgentResult:
        return await self._wait()

    async def cancel(self) -> None:
        self._task.cancel()
        with suppress(asyncio.CancelledError):
            await self._task

    async def _wait(self) -> AgentResult:
        try:
            return await self._task
        except asyncio.CancelledError:
            return AgentResult(status="cancelled", summary="OpenAgent runner was cancelled.")


class OpenAgentRunner:
    """Adapt an OpenAgent AgentLoop into the swarm AgentRunner protocol."""

    def __init__(
        self,
        *,
        runner_id: str = "openagent",
        roles: list[str] | None = None,
        model: LanguageModel,
        model_metadata: Model,
        workspace_root: str | Path,
        system_prompt: str | None = None,
        tools: OpenAgentTools = "readonly",
        permission: PermissionRulesetName | None = None,
        options: dict[str, Any] | None = None,
        toolkit_factory: ToolkitFactory | None = None,
        permission_manager_factory: PermissionManagerFactory | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> None:
        self.model = model
        self.model_metadata = model_metadata
        self.workspace_root = Path(workspace_root).resolve()
        self.system_prompt = system_prompt or _default_system_prompt()
        self.tools = tools
        self.permission = permission
        self.options = dict(options or {})
        self.toolkit_factory = toolkit_factory
        self.permission_manager_factory = permission_manager_factory
        self._descriptor = AgentDescriptor(
            id=runner_id,
            roles=list(roles or ["*"]),
            tool_groups=_tool_groups(tools),
            model_tier="worker",
            max_context=int(model_metadata.context_window),
            supports_streaming=True,
            kind="openagent",
            metadata={
                "model_id": model_metadata.id,
                "provider_id": model_metadata.provider_id,
                **dict(metadata or {}),
            },
        )

    @classmethod
    def from_config(
        cls,
        config: RunnerConfig,
        *,
        model: LanguageModel,
        model_metadata: Model,
        workspace_root: str | Path,
        toolkit_factory: ToolkitFactory | None = None,
        permission_manager_factory: PermissionManagerFactory | None = None,
    ) -> "OpenAgentRunner":
        metadata = dict(config.metadata or {})
        tools = metadata.pop("tools", "readonly")
        if tools != "all" and tools != "readonly" and not isinstance(tools, list):
            tools = "readonly"
        permission = metadata.pop("permission", None)
        system_prompt = metadata.pop("system_prompt", None)
        return cls(
            runner_id=config.id,
            roles=list(config.roles),
            model=model,
            model_metadata=model_metadata,
            workspace_root=workspace_root,
            system_prompt=str(system_prompt) if system_prompt else None,
            tools=tools,  # type: ignore[arg-type]
            permission=str(permission) if permission else None,  # type: ignore[arg-type]
            options=metadata.pop("options", {}),
            toolkit_factory=toolkit_factory,
            permission_manager_factory=permission_manager_factory,
            metadata=metadata,
        )

    @property
    def descriptor(self) -> AgentDescriptor:
        return self._descriptor

    async def start(self, spec: AgentSpec, ctx: RunContext) -> OpenAgentRunHandle:
        spec.validate()
        events: list[AgentEvent] = []
        task = asyncio.create_task(self._run(spec=spec, ctx=ctx, events=events))
        return OpenAgentRunHandle(_task=task, _events=events)

    async def _run(self, *, spec: AgentSpec, ctx: RunContext, events: list[AgentEvent]) -> AgentResult:
        started_at = time.time()
        events.append(
            AgentEvent(
                type="runner.started",
                run_id=ctx.run_id,
                runner_id=self.descriptor.id,
                message="OpenAgent runner started",
                metadata={"role": spec.role, "kind": "openagent"},
            )
        )
        try:
            session = Session(directory=self.workspace_root)
            agent = UniversalAgent(
                config=AgentConfig(
                    name=self.descriptor.id,
                    mode="subagent",
                    model=self.model_metadata,
                    tools=self.tools,
                    permission=self.permission or spec.permissions,
                    max_steps=spec.limits.max_steps or 20,
                    temperature=None,
                    options=self._options_for_spec(spec),
                ),
                model=self.model,
                system_prompt=self.system_prompt,
            )
            loop = AgentLoop(
                agent=agent,
                session=session,
                permission_manager=(self.permission_manager_factory or PermissionManager)(),
                toolkit=self.toolkit_factory() if self.toolkit_factory else None,
            )
            result = await self._run_loop(loop=loop, spec=spec, ctx=ctx, events=events, started_at=started_at)
        except Exception as error:  # noqa: BLE001
            result = AgentResult(
                status="failed",
                summary=str(error),
                metadata={"error_kind": "openagent_runner_error", "runner_id": self.descriptor.id},
            )

        events.append(
            AgentEvent(
                type="runner.finished",
                run_id=ctx.run_id,
                runner_id=self.descriptor.id,
                message=result.summary,
                metadata={"status": result.status, "confidence": result.confidence},
            )
        )
        return result

    async def _run_loop(
        self,
        *,
        loop: AgentLoop,
        spec: AgentSpec,
        ctx: RunContext,
        events: list[AgentEvent],
        started_at: float,
    ) -> AgentResult:
        text_parts: list[str] = []
        input_tokens = 0
        output_tokens = 0
        cost = 0.0
        step_count = 0
        tool_call_count = 0
        errors: list[str] = []
        last_event_type = ""

        async for raw_event in loop.run(_instruction_for_spec(spec)):
            event_type = str(raw_event.get("type") or "")
            last_event_type = event_type
            events.append(_agent_event_from_openagent(ctx=ctx, runner_id=self.descriptor.id, raw_event=raw_event))
            if event_type == "text-delta":
                text_parts.append(str(raw_event.get("text") or ""))
            elif event_type == "tool-result":
                tool_call_count += 1
                if raw_event.get("error"):
                    errors.append(str(raw_event.get("error")))
            elif event_type == "step-finish":
                step_count += 1
                tokens = raw_event.get("tokens") if isinstance(raw_event.get("tokens"), dict) else {}
                input_tokens += int(tokens.get("input") or 0)
                output_tokens += int(tokens.get("output") or 0)
                cost += float(raw_event.get("cost") or 0.0)
            elif event_type == "error":
                errors.append(str(raw_event.get("error") or "unknown OpenAgent error"))

        summary = "".join(text_parts).strip()
        if not summary and errors:
            summary = errors[-1]
        if not summary:
            summary = "OpenAgent completed without a final text response."
        status = "failed" if errors and last_event_type == "error" else "completed"
        latency_ms = int((time.time() - started_at) * 1000)
        session_trace = (loop.session.metadata or {}).get("agent_trace", {})
        return AgentResult(
            status=status,  # type: ignore[arg-type]
            summary=summary,
            confidence=0.0 if status == "failed" else 0.7,
            usage=Usage(
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cost=cost,
                steps=step_count,
                latency_ms=latency_ms,
            ),
            metadata={
                "runner_id": self.descriptor.id,
                "session_id": loop.session.id,
                "openagent_trace": session_trace,
                "tool_call_count": tool_call_count,
                "error_count": len(errors),
            },
        )

    def _options_for_spec(self, spec: AgentSpec) -> dict[str, Any]:
        options = dict(self.options)
        options.setdefault("swarm", {})
        if isinstance(options["swarm"], dict):
            options["swarm"] = {
                **options["swarm"],
                "role": spec.role,
                "inputs": spec.inputs,
                "metadata": spec.metadata,
            }
        return options


def build_openagent_registry(
    config: SwarmConfig,
    *,
    model: LanguageModel,
    model_metadata: Model,
    workspace_root: str | Path,
    toolkit_factory: ToolkitFactory | None = None,
    permission_manager_factory: PermissionManagerFactory | None = None,
) -> RunnerRegistry:
    registry = RunnerRegistry()
    for runner_config in config.runners:
        if runner_config.kind != "openagent":
            continue
        registry.register(
            OpenAgentRunner.from_config(
                runner_config,
                model=model,
                model_metadata=model_metadata,
                workspace_root=workspace_root,
                toolkit_factory=toolkit_factory,
                permission_manager_factory=permission_manager_factory,
            )
        )
    return registry


def _instruction_for_spec(spec: AgentSpec) -> str:
    payload = {
        "role": spec.role,
        "objective": spec.objective,
        "context": spec.context,
        "boundaries": spec.boundaries,
        "inputs": spec.inputs,
        "output_schema": spec.output_schema,
    }
    return (
        "[Swarm worker contract]\n"
        "Complete the assigned worker task under the following contract. "
        "Keep the final answer concise and include evidence when available.\n\n"
        f"{json.dumps(payload, ensure_ascii=False, indent=2)}"
    )


def _agent_event_from_openagent(*, ctx: RunContext, runner_id: str, raw_event: dict[str, Any]) -> AgentEvent:
    event_type = str(raw_event.get("type") or "openagent.event")
    metadata = {key: value for key, value in raw_event.items() if key not in {"text", "output"}}
    return AgentEvent(
        type=f"openagent.{event_type}",
        run_id=ctx.run_id,
        runner_id=runner_id,
        message=str(raw_event.get("text") or raw_event.get("error") or ""),
        metadata=metadata,
    )


def _tool_groups(tools: OpenAgentTools) -> list[str]:
    if tools == "all":
        return ["all"]
    if tools == "readonly":
        return ["readonly"]
    return list(tools)


def _default_system_prompt() -> str:
    return (
        "You are an OpenAgent worker inside a swarm runtime. "
        "Follow the worker contract exactly, respect boundaries, and return a compact result."
    )


__all__ = ["OpenAgentRunner", "OpenAgentRunHandle", "build_openagent_registry"]
