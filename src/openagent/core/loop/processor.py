from __future__ import annotations

import asyncio
import json
import time
from datetime import datetime
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ..agent.base import BaseAgent
from ..context_budget import ContextBudgetConfigError, ContextBudgetResult, check_context_budget, format_context_budget_error, load_context_budget_options
from ..context_pack import ContextItem, ContextPackBuilder
from ..context_messages import (
    CONTEXT_COMPACTION_METADATA_KEY,
    build_brief_messages_for_model,
    build_brief_trimmed_messages_for_model,
    build_messages_for_model,
    build_trimmed_messages_for_model,
    count_new_messages_since_compaction,
    get_context_compaction,
    project_tool_result_to_message,
    prune_old_tool_messages,
    recent_user_turn_start,
)
from ..context_state import build_compaction_record
from ..execution import build_workspace_runtime
from ..file_context import FileContextState
from ..instructions import InstructionContextLoader
from ..observability import ObservationRecorder
from ..permission.manager import PermissionAskRequiredError, PermissionDeniedError, PermissionManager
from ..permission.ruleset import PermissionRuleset
from ..question import QuestionManager, QuestionRequest
from ..runtime_logging import RuntimeLogger
from ..tool.middleware import logging_middleware, observability_middleware, permission_middleware
from ..tool.toolkit import ToolkitAdapter
from ..tool_policy import (
    ToolCapability,
    actionable_missing_capabilities,
    classify_tool_policy,
    format_missing_tools_error,
    format_tool_policy_reminder,
    format_tool_policy_retry_error,
    looks_like_clarification_request,
    missing_required_tools,
    recent_failed_required_tools,
    should_accept_tool_calls,
)
from ..types import ChatMessage, FinishReason, SessionStatus, StreamEvent, ToolResult, ToolSchema, Usage
from ...adapter.memory_adapter import MemoryAdapter
from .doom_loop import DoomLoopDetector
from .snapshot import SnapshotManager

COMPACTION_SYSTEM_PROMPT = (
    "You condense coding sessions into a structured work state so a later model call can continue with minimal loss. "
    "Return only a JSON object. Do not wrap it in markdown."
)
COMPACTION_USER_PROMPT = (
    "Create a structured work state for continuing the conversation above. Return exactly one JSON object with this shape:\n"
    "{\n"
    '  "task": "current goal and user intent",\n'
    '  "progress": ["completed work and durable facts"],\n'
    '  "decisions": ["decisions, constraints, assumptions, or user preferences"],\n'
    '  "files": [{"path": "relative/or/display/path", "status": "read|modified|created|deleted|mentioned|unknown", "note": "why it matters"}],\n'
    '  "tool_findings": ["important tool results, errors, evidence, or commands run"],\n'
    '  "todos": ["active todo items, preserving status when useful"],\n'
    '  "open_questions": ["questions that still need user input"],\n'
    '  "blockers": ["external blockers or missing information"],\n'
    '  "next_steps": ["most likely next actions"],\n'
    '  "risks": ["verification gaps or risks to mention later"]\n'
    "}\n"
    "Preserve concrete filenames, commands, errors, design decisions, and test results. "
    "Drop stale chatter, repeated attempts, and generic politeness. Keep each item compact but specific."
)
MAX_STEPS_TEXT_ONLY_PROMPT = (
    "CRITICAL - MAXIMUM STEPS REACHED\n\n"
    "This is the final allowed model turn for this request. Tools are disabled.\n"
    "Respond with text only and provide the best final answer possible using the work completed so far.\n"
    "Summarize what was accomplished, give the most useful result you can, and mention any important remaining gaps "
    "or next steps only if something is still unfinished."
)
CONTEXT_OVERFLOW_TEXT_ONLY_PROMPT = (
    "CONTEXT OVERFLOW RECOVERY\n\n"
    "The conversation history was trimmed to fit the model context window. Tools are disabled for this attempt.\n"
    "Respond with text only using the remaining context. Summarize the most important completed work, provide the "
    "best available answer, and call out only the most important missing information or next step if needed."
)
WEB_SEARCH_TOOL_NAME = "web_search"
WEB_FETCH_TOOL_NAME = "web_fetch"
WEB_SEARCH_SUCCESS_THRESHOLD = 3
WEB_FETCH_FAILURE_THRESHOLD = 2
CONTEXT_PACK_TRACE_METADATA_KEY = "context_pack_trace"
LAST_CONTEXT_PACK_METADATA_KEY = "last_context_pack"
CONTEXT_PACK_TRACE_LIMIT = 20


def _projection_for_fallback_stage(stage: str) -> str:
    if stage in {"initial", "after_prune", "after_compact"}:
        return "full"
    if stage == "after_compact_brief":
        return "brief"
    if stage in {"after_compact_minimal", "after_trim"}:
        return "minimal"
    if stage == "final_text_only":
        return "text_only"
    if stage == "current_user_only":
        return "current_user_only"
    return "unknown"


@dataclass(slots=True)
class AgentLoopConfig:
    max_steps: int = 50
    doom_loop_threshold: int = 3
    max_retry: int = 2
    retry_base_delay_s: float = 1.0


@dataclass(frozen=True, slots=True)
class PreparedModelCall:
    messages: list[ChatMessage]
    tools: list[ToolSchema]
    budget: ContextBudgetResult | None
    max_output_tokens: int | None = None
    overflow_text_only: bool = False


@dataclass(frozen=True, slots=True)
class ToolFailureHint:
    kind: str
    tool_name: str


@dataclass(slots=True)
class WebResearchState:
    successful_web_search_count: int = 0
    failed_web_fetch_count: int = 0
    web_search_quota_blocked: bool = False
    convergence_reminder_used: bool = False

    @property
    def fetch_failure_converged(self) -> bool:
        return (
            self.failed_web_fetch_count >= WEB_FETCH_FAILURE_THRESHOLD
            and self.successful_web_search_count >= 1
        )

    @property
    def web_search_converged(self) -> bool:
        return self.successful_web_search_count >= WEB_SEARCH_SUCCESS_THRESHOLD

    @property
    def disable_web_search(self) -> bool:
        return self.web_search_quota_blocked or self.web_search_converged or self.fetch_failure_converged

    @property
    def disable_web_fetch(self) -> bool:
        return self.fetch_failure_converged

    @property
    def needs_reminder(self) -> bool:
        return (self.disable_web_search or self.disable_web_fetch) and not self.convergence_reminder_used


class AgentLoop:
    def __init__(
        self,
        *,
        agent: BaseAgent,
        session,
        permission_manager: PermissionManager,
        toolkit: ToolkitAdapter | None = None,
        snapshot_manager: SnapshotManager | None = None,
        doom_loop_detector: DoomLoopDetector | None = None,
        config: AgentLoopConfig | None = None,
        question_manager: QuestionManager | None = None,
    ) -> None:
        self.agent = agent
        self.session = session
        self.permission_manager = permission_manager
        self.config = config or AgentLoopConfig(max_steps=agent.config.max_steps)
        self.snapshot_manager = snapshot_manager or SnapshotManager()
        self.doom_loop_detector = doom_loop_detector or DoomLoopDetector(self.config.doom_loop_threshold)
        self.toolkit = toolkit or ToolkitAdapter()
        self.workspace_runtime = build_workspace_runtime(session)
        self.memory = MemoryAdapter()
        self.tool_log: list[dict[str, Any]] = []
        self.observation_recorder: ObservationRecorder | None = None
        self.runtime_logger: RuntimeLogger | None = None
        self.question_manager = question_manager or QuestionManager()
        self.question_manager.set_hooks(on_requested=self._on_question_requested, on_resolved=self._on_question_resolved)
        self._init_tools()

    def _tools_for_agent(self) -> list[ToolSchema]:
        if self.agent.config.permission == "NONE":
            return []

        tools = self.toolkit.get_all_tools(execution_mode=self.workspace_runtime.mode)
        allow = self.agent.config.tools
        if allow == "all":
            return tools
        if allow == "readonly":
            allowed_names = {"read", "glob", "grep", "ls", "skill", "todoread", "question"}
            return [tool for tool in tools if tool.name in allowed_names]
        if isinstance(allow, list):
            allowed_names = set(allow)
            return [tool for tool in tools if tool.name in allowed_names]
        return tools

    def _filter_web_tools_for_state(self, tools: list[ToolSchema], state: WebResearchState) -> list[ToolSchema]:
        if not state.disable_web_search and not state.disable_web_fetch:
            return tools

        filtered: list[ToolSchema] = []
        for tool in tools:
            if state.disable_web_search and tool.name == WEB_SEARCH_TOOL_NAME:
                continue
            if state.disable_web_fetch and tool.name == WEB_FETCH_TOOL_NAME:
                continue
            filtered.append(tool)
        return filtered

    def _update_web_research_state(self, *, tool_name: str, result: ToolResult, state: WebResearchState) -> None:
        metadata = result.metadata or {}
        error_kind = str(metadata.get("error_kind") or "")
        if tool_name == WEB_SEARCH_TOOL_NAME:
            if result.error:
                error_text = str(result.error).lower()
                if error_kind == "web_search_quota" or any(
                    marker in error_text
                    for marker in ("quota", "rate limit", "free credit", "too many requests", " 429", "(429)")
                ):
                    state.web_search_quota_blocked = True
                return
            returned_count = metadata.get("returned_count", metadata.get("count", 0))
            try:
                has_results = int(returned_count or 0) > 0
            except (TypeError, ValueError):
                has_results = bool(result.output)
            if has_results:
                state.successful_web_search_count += 1
            return

        if tool_name == WEB_FETCH_TOOL_NAME and result.error:
            state.failed_web_fetch_count += 1

    def _web_research_convergence_message(self, state: WebResearchState) -> ChatMessage | None:
        if not state.needs_reminder:
            return None

        lines = ["[Web research convergence]"]
        if state.web_search_quota_blocked:
            lines.append(
                "web_search is currently blocked by quota or rate limits. Do not call web_search again in this turn."
            )
            if state.successful_web_search_count > 0:
                lines.append(
                    "Use the evidence already gathered and produce a bounded answer, clearly naming any remaining gaps."
                )
            else:
                lines.append(
                    "If no usable evidence is available, say that search quota is unavailable and ask for configured search credentials or source URLs."
                )
        elif state.fetch_failure_converged:
            lines.append(
                "web_fetch has failed repeatedly after search evidence was already collected. Stop expanding web research with web_search or web_fetch."
            )
            lines.append("Use existing evidence, answer with caveats, and explicitly state any missing source detail.")
        elif state.web_search_converged:
            lines.append(
                "Enough web_search evidence has been collected for this turn. Do not call web_search again."
            )
            lines.append("Synthesize from the gathered evidence and only use non-web tools if they are still necessary.")

        state.convergence_reminder_used = True
        return ChatMessage(
            role="assistant",
            content="\n".join(lines),
            metadata={"synthetic": True, "web_research_convergence": True},
        )

    def _init_tools(self) -> None:
        self.toolkit.register_middleware(permission_middleware(self.permission_manager))
        self.toolkit.register_middleware(observability_middleware())
        self.toolkit.register_middleware(logging_middleware(self.tool_log))
        self.toolkit.load_builtin()

        tool_paths = self.agent.config.options.get("tool_paths")
        if isinstance(tool_paths, list) and all(isinstance(item, str) for item in tool_paths):
            self.toolkit.load_plugins(tool_paths=tool_paths, base_dir=Path(self.session.directory))

    def _context_budget_config(self) -> dict[str, Any]:
        return load_context_budget_options(self.agent.config.options, model=self.agent.config.model)

    def _record_budget_diagnostics(self, budget: ContextBudgetResult | None) -> None:
        if budget is None:
            return
        config = self._context_budget_config()
        diagnostics = {
            "estimated_input_tokens": budget.estimated_input_tokens,
            "input_limit_tokens": budget.input_limit_tokens,
            "context_window": budget.context_window,
            "reserved_output_tokens": budget.reserved_output_tokens,
            "overflowed": budget.overflowed,
            "tool_message_count": budget.tool_message_count,
            "largest_tool_message_tokens": budget.largest_tool_message_tokens,
            "largest_tool_message_name": budget.largest_tool_message_name,
            "counting_method": budget.counting_method,
            "counting_exact": budget.counting_exact,
            "fallback_stage": budget.fallback_stage,
            "payload_kind": budget.payload_kind,
            "compaction_mode": config.get("compaction_mode"),
        }
        self.session.metadata["last_context_budget"] = diagnostics
        self._append_context_projection_diagnostic(budget=budget, config=config)
        self._record_observation(
            "context.budget_checked",
            kind="context",
            attributes={
                **diagnostics,
                "projection": _projection_for_fallback_stage(budget.fallback_stage),
                "strategy": config.get("strategy"),
                "prune_old_tool_outputs": bool(config.get("prune_old_tool_outputs")),
            },
        )

    def _append_context_projection_diagnostic(self, *, budget: ContextBudgetResult, config: dict[str, Any]) -> None:
        trace_raw = self.session.metadata.get("context_projection_trace")
        trace = list(trace_raw) if isinstance(trace_raw, list) else []
        trace.append(
            {
                "stage": budget.fallback_stage,
                "projection": _projection_for_fallback_stage(budget.fallback_stage),
                "estimated_input_tokens": budget.estimated_input_tokens,
                "input_limit_tokens": budget.input_limit_tokens,
                "overflowed": budget.overflowed,
                "tool_message_count": budget.tool_message_count,
                "largest_tool_message_tokens": budget.largest_tool_message_tokens,
                "largest_tool_message_name": budget.largest_tool_message_name,
                "compaction_mode": config.get("compaction_mode"),
                "prune_old_tool_outputs": bool(config.get("prune_old_tool_outputs")),
                "reserved_output_tokens": budget.reserved_output_tokens,
            }
        )
        self.session.metadata["context_projection_trace"] = trace[-20:]

    def _sandbox_metadata_for_context_pack(self) -> dict[str, Any]:
        execution = dict(getattr(self.workspace_runtime, "execution_metadata", {}) or {})
        session_execution = self.session.metadata.get("execution")
        if isinstance(session_execution, dict):
            execution.update(session_execution)
        if "mode" not in execution and execution.get("execution_mode") is not None:
            execution["mode"] = execution.get("execution_mode")
        return execution

    @staticmethod
    def _runtime_context_from_messages(messages: list[ChatMessage]) -> str | None:
        for message in reversed(messages):
            if bool((message.metadata or {}).get("runtime_context")):
                return message.content
        return None

    def _instruction_context_items(self) -> list[ContextItem]:
        try:
            instruction_context = InstructionContextLoader(self.session.directory).load()
        except Exception as error:  # noqa: BLE001
            self.session.metadata["last_instruction_context"] = {
                "item_count": 0,
                "total_bytes": 0,
                "truncated": False,
                "issues": [f"load_failed:{type(error).__name__}"],
            }
            return []

        self.session.metadata["last_instruction_context"] = {
            "item_count": len(instruction_context.items),
            "total_bytes": instruction_context.total_bytes,
            "truncated": instruction_context.truncated,
            "issues": list(instruction_context.issues),
            "items": [
                {
                    "source": item.source,
                    "scope": item.scope,
                    "bytes_read": item.bytes_read,
                    "truncated": item.truncated,
                }
                for item in instruction_context.items
            ],
        }
        return instruction_context.to_context_items()

    def _file_context_items(self) -> list[ContextItem]:
        return FileContextState.from_metadata(self.session.metadata).to_context_items()

    def _record_context_pack_diagnostics(
        self,
        *,
        messages: list[ChatMessage],
        fallback_stage: str,
        step_index: int,
    ) -> None:
        runtime_context = self._runtime_context_from_messages(messages)
        pack_messages = [
            message
            for message in messages
            if not bool((message.metadata or {}).get("runtime_context"))
        ]
        pack = ContextPackBuilder().build(
            messages=pack_messages,
            metadata=self.session.metadata,
            todos=self.session.todos,
            runtime_context=runtime_context,
            sandbox_metadata=self._sandbox_metadata_for_context_pack(),
            extra_items=[*self._instruction_context_items(), *self._file_context_items()],
        )
        trace_items = pack.trace_dicts()
        diagnostic = {
            "step_index": step_index,
            "fallback_stage": fallback_stage,
            "message_count": len(messages),
            "item_count": len(pack.items),
            "included_count": sum(1 for item in trace_items if item.get("included")),
            "estimated_input_tokens": pack.estimated_input_tokens,
            "items": trace_items,
        }
        trace_raw = self.session.metadata.get(CONTEXT_PACK_TRACE_METADATA_KEY)
        trace = list(trace_raw) if isinstance(trace_raw, list) else []
        trace.append(diagnostic)
        self.session.metadata[CONTEXT_PACK_TRACE_METADATA_KEY] = trace[-CONTEXT_PACK_TRACE_LIMIT:]
        self.session.metadata[LAST_CONTEXT_PACK_METADATA_KEY] = diagnostic
        self._record_observation(
            "context.pack_built",
            kind="context",
            attributes={
                "step_index": step_index,
                "fallback_stage": fallback_stage,
                "message_count": len(messages),
                "item_count": len(pack.items),
                "included_count": diagnostic["included_count"],
                "estimated_input_tokens": pack.estimated_input_tokens,
            },
        )

    def _record_model_usage(self, usage: Usage) -> None:
        self.session.metadata["last_model_usage"] = {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "cost": usage.cost,
        }
        self.session.metadata["last_model_usage_at"] = int(time.time() * 1000)
        self._record_observation(
            "model.usage",
            kind="model",
            attributes={
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "cost": usage.cost,
            },
        )

    def _last_usage_needs_preemptive_reduction(self) -> bool:
        usage = self.session.metadata.get("last_model_usage")
        budget = self.session.metadata.get("last_context_budget")
        if not isinstance(usage, dict) or not isinstance(budget, dict):
            return False
        input_tokens = int(usage.get("input_tokens", 0))
        input_limit_tokens = int(budget.get("input_limit_tokens", 0))
        if input_limit_tokens <= 0:
            return False
        return input_tokens >= max(int(input_limit_tokens * 0.9), input_limit_tokens - 512)

    def _invalidate_context_compaction_if_needed(self) -> None:
        compaction = get_context_compaction(self.session.metadata, message_count=len(self.session.messages))
        if compaction is None:
            self.session.metadata.pop(CONTEXT_COMPACTION_METADATA_KEY, None)

    def _runtime_context_message(self) -> ChatMessage:
        now = datetime.now().astimezone()
        offset = now.strftime("%z")
        offset_label = f"UTC{offset[:3]}:{offset[3:]}" if offset else "UTC"
        lines = [
            "[Runtime context]",
            f"Current local datetime: {now.strftime('%Y-%m-%d %H:%M:%S')} {offset_label}",
            "Resolve today, tomorrow, yesterday, and this week from this runtime timestamp, not memory.",
        ]
        return ChatMessage(
            role="assistant",
            content="\n".join(lines),
            metadata={"synthetic": True, "runtime_context": True},
        )

    def _append_runtime_context(self, messages: list[ChatMessage]) -> list[ChatMessage]:
        filtered = [message for message in messages if not bool((message.metadata or {}).get("runtime_context"))]
        return [*filtered, self._runtime_context_message()]

    def _messages_for_model(self) -> list[ChatMessage]:
        self._invalidate_context_compaction_if_needed()
        return self._append_runtime_context(build_messages_for_model(self.session.messages, self.session.metadata))

    def _messages_for_final_step(self, messages: list[ChatMessage]) -> list[ChatMessage]:
        runtime_messages = self._append_runtime_context(messages)
        return [
            *runtime_messages,
            ChatMessage(role="assistant", content=MAX_STEPS_TEXT_ONLY_PROMPT, metadata={"synthetic": True}),
        ]

    def _messages_for_overflow_final_attempt(self, messages: list[ChatMessage]) -> list[ChatMessage]:
        runtime_messages = self._append_runtime_context(messages)
        return [
            *runtime_messages,
            ChatMessage(role="assistant", content=CONTEXT_OVERFLOW_TEXT_ONLY_PROMPT, metadata={"synthetic": True, "overflow": True}),
        ]

    def _check_budget(
        self,
        *,
        messages: list[ChatMessage],
        tools: list[ToolSchema],
        fallback_stage: str,
        reserve_output_tokens_override: int | None = None,
    ) -> ContextBudgetResult | None:
        options = self._context_budget_options_override(reserve_output_tokens_override=reserve_output_tokens_override)
        budget = check_context_budget(
            system=self.agent.system_prompt,
            messages=messages,
            tools=tools,
            model=self.agent.config.model,
            options=options,
            fallback_stage=fallback_stage,
        )
        self._record_budget_diagnostics(budget)
        return budget

    def _context_budget_options_override(self, *, reserve_output_tokens_override: int | None = None) -> dict[str, Any]:
        if reserve_output_tokens_override is None:
            return self.agent.config.options

        options = dict(self.agent.config.options)
        raw_context_budget = options.get("context_budget")
        context_budget = dict(raw_context_budget) if isinstance(raw_context_budget, dict) else {}
        context_budget["reserve_output_tokens"] = int(reserve_output_tokens_override)
        options["context_budget"] = context_budget
        return options

    def _apply_tool_output_pruning(self, config: dict[str, Any]) -> int:
        if not bool(config["prune_old_tool_outputs"]):
            return 0

        new_messages, reclaimed = prune_old_tool_messages(
            self.session.messages,
            bytes_per_token=int(config["bytes_per_token"]),
            keep_recent_user_turns=int(config["prune_keep_recent_user_turns"]),
            protect_input_tokens=int(config["prune_protect_input_tokens"]),
            min_input_tokens=int(config["prune_min_input_tokens"]),
            model=self.agent.config.model,
            options=self.agent.config.options,
            counting=str(config["counting"]),
        )
        if reclaimed > 0:
            self.session.messages = new_messages
            self._log_runtime(
                "INFO",
                "Old tool output pruned from context",
                category="context",
                attributes={"reclaimed_tokens": reclaimed},
            )
        self.session.metadata["last_tool_output_prune_tokens"] = reclaimed
        return reclaimed

    def _record_observation(
        self,
        name: str,
        *,
        kind: str = "event",
        status: str = "ok",
        attributes: dict[str, Any] | None = None,
        duration_ms: int | None = None,
    ) -> None:
        if self.observation_recorder is None:
            return
        self.observation_recorder.event(
            name,
            kind=kind,
            status=status,
            attributes=attributes,
            duration_ms=duration_ms,
        )

    def _log_runtime(
        self,
        level: str,
        message: str,
        *,
        category: str = "runtime",
        attributes: dict[str, Any] | None = None,
    ) -> None:
        if self.runtime_logger is None:
            return
        self.runtime_logger.log(level, message, category=category, attributes=attributes)

    def _mark_observed_error(
        self,
        span,
        *,
        message: str,
        error_kind: str,
        step_index: int | None = None,
        attempt_index: int | None = None,
    ) -> None:
        if span is not None:
            span.status = "error"
            span.error = {
                "type": error_kind,
                "message": message,
                "error_kind": error_kind,
            }
        self._record_observation(
            "run.failed",
            kind="run",
            status="error",
            attributes={
                "error_kind": error_kind,
                "message": message,
                "step_index": step_index,
                "attempt_index": attempt_index,
            },
        )
        self._record_observation(
            "error",
            kind="error",
            status="error",
            attributes={
                "error_kind": error_kind,
                "message": message,
                "step_index": step_index,
                "attempt_index": attempt_index,
            },
        )
        self._log_runtime(
            "ERROR",
            message,
            category="error",
            attributes={
                "error_kind": error_kind,
                "step_index": step_index,
                "attempt_index": attempt_index,
            },
        )

    def _new_observation_recorder(self) -> ObservationRecorder:
        model = self.agent.config.model
        return ObservationRecorder.for_session(
            session_id=self.session.id,
            session_metadata=self.session.metadata,
            agent_name=self.agent.config.name,
            model_id=model.id if model is not None else None,
            provider_id=model.provider_id if model is not None else None,
            workspace=str(self.session.directory),
            options=self.agent.config.options,
            base_dir=Path(self.session.directory),
        )

    def _new_runtime_logger(self) -> RuntimeLogger:
        return RuntimeLogger.for_session(
            session_id=self.session.id,
            session_metadata=self.session.metadata,
            options=self.agent.config.options,
            base_dir=Path(self.session.directory),
        )

    async def _run_compaction_model(self, messages_to_compact: list[ChatMessage], *, max_output_tokens: int) -> str:
        todo_payload = [todo.to_dict() for todo in self.session.todos]
        todo_section = ""
        if todo_payload:
            todo_section = "\nCurrent todo list:\n" + json.dumps(todo_payload, ensure_ascii=False)

        prompt_messages = [
            *self._append_runtime_context(messages_to_compact),
            ChatMessage(role="user", content=COMPACTION_USER_PROMPT + todo_section),
        ]
        chunks: list[str] = []
        recorder = self.observation_recorder
        if recorder is None:
            async for event in self.agent.model.stream(
                system=COMPACTION_SYSTEM_PROMPT,
                messages=prompt_messages,
                tools=[],
                temperature=self.agent.config.temperature,
                max_output_tokens=max_output_tokens,
                options=self.agent.config.options,
            ):
                if event.get("type") == "text-delta":
                    chunks.append(str(event.get("text", "")))
            return "".join(chunks).strip()

        with recorder.span(
            "context.compaction",
            kind="context",
            attributes={
                "message_count": len(messages_to_compact),
                "max_output_tokens": max_output_tokens,
            },
        ) as span:
            async for event in self.agent.model.stream(
                system=COMPACTION_SYSTEM_PROMPT,
                messages=prompt_messages,
                tools=[],
                temperature=self.agent.config.temperature,
                max_output_tokens=max_output_tokens,
                options=self.agent.config.options,
            ):
                if event.get("type") == "text-delta":
                    chunks.append(str(event.get("text", "")))
            raw = "".join(chunks).strip()
            span.set_attribute("output_chars", len(raw))
            return raw

    async def _compact_context(self, config: dict[str, Any]) -> bool:
        compacted_until = recent_user_turn_start(self.session.messages, int(config["prune_keep_recent_user_turns"]))
        if compacted_until <= 0:
            self._log_runtime(
                "DEBUG",
                "Context compaction skipped",
                category="context",
                attributes={"reason": "not_enough_history", "compacted_until": compacted_until},
            )
            return False

        messages_to_compact = list(self.session.messages[:compacted_until])
        if not messages_to_compact:
            self._log_runtime(
                "DEBUG",
                "Context compaction skipped",
                category="context",
                attributes={"reason": "empty_history"},
            )
            return False
        self._log_runtime(
            "INFO",
            "Context compaction started",
            category="context",
            attributes={
                "message_count": len(messages_to_compact),
                "compacted_until": compacted_until,
                "mode": config.get("compaction_mode"),
            },
        )

        raw_compaction = await self._run_compaction_model(
            messages_to_compact,
            max_output_tokens=int(config["compact_summary_max_output_tokens"]),
        )
        if not raw_compaction:
            self._log_runtime(
                "WARNING",
                "Context compaction produced empty output",
                category="context",
                attributes={"compacted_until": compacted_until},
            )
            self._record_observation(
                "context.compaction.finished",
                kind="context",
                attributes={"compacted": False, "reason": "empty_model_output"},
            )
            return False

        record = build_compaction_record(
            raw_text=raw_compaction,
            compacted_until=compacted_until,
            updated_at=int(time.time() * 1000),
        )
        self.session.metadata[CONTEXT_COMPACTION_METADATA_KEY] = record
        self._record_observation(
            "context.compaction.finished",
            kind="context",
            attributes={
                "compacted": True,
                "source": record.get("source"),
                "format": record.get("format"),
                "compacted_until": compacted_until,
            },
        )
        self._log_runtime(
            "INFO",
            "Context compaction finished",
            category="context",
            attributes={
                "source": record.get("source"),
                "format": record.get("format"),
                "compacted_until": compacted_until,
            },
        )
        return True

    async def _maybe_refresh_context_compaction(self, config: dict[str, Any], *, force: bool = False) -> None:
        if config["strategy"] not in {"compact", "auto"}:
            return
        compaction = get_context_compaction(self.session.metadata, message_count=len(self.session.messages))
        if compaction is None:
            if force:
                await self._compact_context(config)
            return
        if not force and count_new_messages_since_compaction(self.session.messages, self.session.metadata) < int(
            config["compact_refresh_min_new_messages"]
        ):
            return
        await self._compact_context(config)

    def _build_overflow_trimmed_messages(self, config: dict[str, Any]) -> list[ChatMessage]:
        self._invalidate_context_compaction_if_needed()
        return build_trimmed_messages_for_model(
            self.session.messages,
            self.session.metadata,
            keep_recent_user_turns=int(config["overflow_keep_recent_user_turns"]),
            compact_tool_messages=True,
        )

    def _current_user_only_budget_error(self) -> str | None:
        current_user = next((message for message in reversed(self.session.messages) if message.role == "user"), None)
        if current_user is None:
            return None
        budget = self._check_budget(messages=[current_user], tools=[], fallback_stage="current_user_only")
        if budget is None or not budget.overflowed:
            return None
        return "Current user input is too large to fit within the model context even after overflow trimming: " + format_context_budget_error(budget)

    async def _prepare_messages_for_model(
        self,
        *,
        tools: list[ToolSchema],
    ) -> tuple[PreparedModelCall | None, str | None, dict[str, Any] | None]:
        try:
            config = self._context_budget_config()
        except ContextBudgetConfigError as error:
            return None, str(error), None

        if self._last_usage_needs_preemptive_reduction():
            self._apply_tool_output_pruning(config)
            if config["strategy"] in {"compact", "auto"}:
                try:
                    await self._maybe_refresh_context_compaction(config, force=True)
                except Exception:
                    pass

        messages = self._messages_for_model()
        budget = self._check_budget(messages=messages, tools=tools, fallback_stage="initial")
        if budget is None or not budget.overflowed:
            return PreparedModelCall(messages=messages, tools=tools, budget=budget), None, config

        if config["strategy"] == "error":
            return None, format_context_budget_error(budget), config

        pruned_tokens = self._apply_tool_output_pruning(config)
        if pruned_tokens > 0:
            messages = self._messages_for_model()
            budget = self._check_budget(messages=messages, tools=tools, fallback_stage="after_prune")
            if budget is None or not budget.overflowed:
                return PreparedModelCall(messages=messages, tools=tools, budget=budget), None, config

        compacted = False
        if config["strategy"] in {"compact", "auto"}:
            try:
                compacted = await self._compact_context(config)
            except Exception:
                compacted = False
            if compacted:
                messages = self._messages_for_model()
                budget = self._check_budget(messages=messages, tools=tools, fallback_stage="after_compact")
                if budget is None or not budget.overflowed:
                    return PreparedModelCall(messages=messages, tools=tools, budget=budget), None, config
                brief_messages = self._append_runtime_context(
                    build_brief_messages_for_model(self.session.messages, self.session.metadata)
                )
                brief_budget = self._check_budget(messages=brief_messages, tools=tools, fallback_stage="after_compact_brief")
                if brief_budget is None or not brief_budget.overflowed:
                    return PreparedModelCall(messages=brief_messages, tools=tools, budget=brief_budget), None, config
                minimal_messages = self._append_runtime_context(
                    build_brief_trimmed_messages_for_model(
                        self.session.messages,
                        self.session.metadata,
                        keep_recent_user_turns=1,
                    )
                )
                minimal_budget = self._check_budget(
                    messages=minimal_messages,
                    tools=tools,
                    fallback_stage="after_compact_minimal",
                )
                if minimal_budget is None or not minimal_budget.overflowed:
                    return PreparedModelCall(messages=minimal_messages, tools=tools, budget=minimal_budget), None, config

        if config["strategy"] == "compact":
            return None, format_context_budget_error(budget), config

        trimmed_messages = self._build_overflow_trimmed_messages(config)
        trimmed_budget = self._check_budget(messages=trimmed_messages, tools=tools, fallback_stage="after_trim")
        if trimmed_budget is None or not trimmed_budget.overflowed:
            return PreparedModelCall(messages=trimmed_messages, tools=tools, budget=trimmed_budget), None, config

        final_tools = [] if bool(config["overflow_disable_tools_on_final_attempt"]) else tools
        final_messages = self._messages_for_overflow_final_attempt(trimmed_messages)
        final_budget = self._check_budget(
            messages=final_messages,
            tools=final_tools,
            fallback_stage="final_text_only",
            reserve_output_tokens_override=int(config["overflow_final_max_output_tokens"]),
        )
        if final_budget is None or not final_budget.overflowed:
            return (
                PreparedModelCall(
                    messages=final_messages,
                    tools=final_tools,
                    budget=final_budget,
                    max_output_tokens=int(config["overflow_final_max_output_tokens"]),
                    overflow_text_only=True,
                ),
                None,
                config,
            )

        current_user_error = self._current_user_only_budget_error()
        if current_user_error is not None:
            return None, current_user_error, config
        return None, format_context_budget_error(final_budget), config

    def _repeated_tool_call_error(self, call) -> str:
        rendered_input = json.dumps(call.input, ensure_ascii=False, sort_keys=True)
        return (
            f"Detected repeated tool-call loop (threshold={self.config.doom_loop_threshold}): "
            f"{call.name} {rendered_input}"
        )

    def _on_question_requested(self, _request: QuestionRequest) -> None:
        self.session.status = SessionStatus.PAUSED

    def _on_question_resolved(self, _request: QuestionRequest) -> None:
        self.session.status = SessionStatus.RUNNING

    def _question_request_event(self, request: QuestionRequest) -> dict[str, Any]:
        return {
            "type": "question-request",
            "request_id": request.request_id,
            "session_id": request.session_id,
            "tool_call_id": request.tool_call_id,
            "questions": [
                {
                    "header": question.header,
                    "question": question.question,
                    "multiple": question.multiple,
                    "options": [
                        {"label": option.label, "description": option.description}
                        for option in question.options
                    ],
                }
                for question in request.questions
            ],
        }

    def _tool_context(self) -> dict[str, Any]:
        execution_metadata = dict(getattr(self.workspace_runtime, "execution_metadata", {}) or {})
        return {
            "session_id": self.session.id,
            "session_root": str(self.session.directory),
            "memory": self.memory,
            "session": self.session,
            "question_manager": self.question_manager,
            "agent_options": self.agent.config.options,
            "execution_mode": self.workspace_runtime.mode,
            "workspace_root": getattr(self.workspace_runtime, "workspace_root", str(self.session.directory)),
            "workspace_runtime": self.workspace_runtime,
            "execution_metadata": execution_metadata,
            "observation_recorder": self.observation_recorder,
            "runtime_logger": self.runtime_logger,
        }

    @staticmethod
    def _tool_error_title(kind: str, tool_name: str) -> str:
        if kind == "permission_denied":
            return f"{tool_name} permission denied"
        if kind == "question_rejected":
            return f"{tool_name} question rejected"
        return f"{tool_name} failed"

    def _build_tool_error_result(
        self,
        *,
        call,
        error: str,
        kind: str,
        metadata: dict[str, Any] | None = None,
    ) -> ToolResult:
        payload = dict(metadata or {})
        payload.setdefault("tool", call.name)
        payload.setdefault("title", self._tool_error_title(kind, call.name))
        payload["error_kind"] = kind
        return ToolResult(call_id=call.call_id, output="", error=error, metadata=payload)

    def _normalize_tool_result_error_kind(self, *, tool_name: str, result: ToolResult) -> ToolResult:
        if not result.error:
            return result
        metadata = dict(result.metadata or {})
        error_kind = metadata.get("error_kind")
        if not isinstance(error_kind, str) or not error_kind:
            error_text = str(result.error).lower()
            if tool_name == "question" and "dismissed" in error_text:
                error_kind = "question_rejected"
            elif "permission" in error_text and "denied" in error_text:
                error_kind = "permission_denied"
            else:
                error_kind = "tool_error"
            metadata["error_kind"] = error_kind
        metadata.setdefault("tool", tool_name)
        metadata.setdefault("title", self._tool_error_title(str(error_kind), tool_name))
        return ToolResult(
            call_id=result.call_id,
            output=result.output,
            error=result.error,
            metadata=metadata,
        )

    def _failure_followup_messages(self, failures: list[ToolFailureHint]) -> list[ChatMessage]:
        if not failures:
            return []
        kinds = {failure.kind for failure in failures}
        tool_names = sorted({failure.tool_name for failure in failures if failure.tool_name})
        lines = [
            "[Tool failure follow-up]",
            "One or more tool calls in the previous step failed.",
        ]
        if "permission_denied" in kinds:
            lines.append(
                "A tool permission request was denied. Do not repeat the same dangerous call unchanged; explain the impact and prefer a safer alternative."
            )
        if "question_rejected" in kinds:
            lines.append(
                "A required user question was dismissed. Continue with the best safe fallback and clearly state what information is still missing."
            )
        if "web_search_quota" in kinds:
            lines.append(
                "web_search hit quota or rate limits. Do not retry the same search path; use existing evidence or explain that configured search credentials/source URLs are needed."
            )
        if any(failure.tool_name == WEB_FETCH_TOOL_NAME for failure in failures):
            lines.append(
                "web_fetch failed. Do not keep broadening web research just because a page fetch failed; use existing search evidence when possible."
            )
        if any(failure.tool_name == WEB_SEARCH_TOOL_NAME for failure in failures) and "web_search_quota" not in kinds:
            lines.append(
                "web_search failed. Avoid repeating equivalent searches; answer from available evidence or state the search gap."
            )
        if "tool_error" in kinds:
            lines.append(
                "A tool execution failed. Prefer summarizing the failure and trying a different source or method instead of blindly retrying the same call."
            )
        if tool_names:
            lines.append("Failed tools: " + ", ".join(tool_names))
        return [ChatMessage(role="assistant", content="\n".join(lines), metadata={"synthetic": True, "tool_failure_followup": True})]

    def _policy_followup_messages(
        self,
        policy,
        missing_capabilities: list[ToolCapability],
        *,
        failed_tools: set[str],
    ) -> list[ChatMessage]:
        if not missing_capabilities:
            return []
        lines = [
            "[Tool policy follow-up]",
            "This request still needs tool-backed work before the final answer.",
            format_tool_policy_reminder(policy, missing_capabilities),
        ]
        if failed_tools:
            lines.append(
                "Avoid blindly retrying the same failed tool. Prefer a different valid tool path, or give a bounded answer if the remaining tool path is blocked."
            )
        return [ChatMessage(role="assistant", content="\n".join(lines), metadata={"synthetic": True, "tool_policy_followup": True})]

    async def run(self, user_text: str):
        self.observation_recorder = self._new_observation_recorder()
        self.runtime_logger = self._new_runtime_logger()
        self.runtime_logger.bind_trace(
            run_id=self.observation_recorder.trace.run_id,
            trace_id=self.observation_recorder.trace.trace_id,
            span_getter=lambda: self.observation_recorder.current_span_id if self.observation_recorder else None,
        )
        self.permission_manager.set_ruleset(PermissionRuleset[self.agent.config.permission])
        self.session.status = SessionStatus.RUNNING
        self.session.add(ChatMessage(role="user", content=user_text))
        self._record_observation(
            "run.started",
            kind="run",
            attributes={
                "agent_name": self.agent.config.name,
                "model_id": self.agent.config.model.id if self.agent.config.model is not None else None,
                "provider_id": self.agent.config.model.provider_id if self.agent.config.model is not None else None,
                "permission": self.agent.config.permission,
                "input_chars": len(user_text),
            },
        )
        self._log_runtime(
            "INFO",
            "Agent run started",
            category="run",
            attributes={
                "agent_name": self.agent.config.name,
                "model_id": self.agent.config.model.id if self.agent.config.model is not None else None,
                "provider_id": self.agent.config.model.provider_id if self.agent.config.model is not None else None,
                "permission": self.agent.config.permission,
                "input_chars": len(user_text),
            },
        )
        tool_policy = classify_tool_policy(user_text) if self.agent.uses_default_system_prompt else None
        followup_messages: list[ChatMessage] = []
        policy_successful_tools: set[str] = set()
        policy_failed_tools: set[str] = recent_failed_required_tools(self.session.messages, tool_policy) if tool_policy is not None else set()
        policy_soft_followup_used = False
        web_research_state = WebResearchState()

        steps = 0
        try:
            while steps < self.config.max_steps:
                steps += 1
                is_last_step = steps >= self.config.max_steps
                snapshot_id = self.snapshot_manager.track(Path(self.session.directory))
                step_started_at = time.time()
                self._record_observation(
                    "step.started",
                    kind="step",
                    attributes={
                        "step_index": steps,
                        "snapshot_id": snapshot_id,
                        "is_last_step": is_last_step,
                    },
                )
                self._log_runtime(
                    "INFO",
                    "Agent step started",
                    category="step",
                    attributes={
                        "step_index": steps,
                        "snapshot_id": snapshot_id,
                        "is_last_step": is_last_step,
                    },
                )
                yield {"type": "step-start", "snapshot_id": snapshot_id}  # type: ignore[misc]
                available_tools = self._filter_web_tools_for_state(self._tools_for_agent(), web_research_state)
                available_tool_names = {tool.name for tool in available_tools}
                requested_tools = [] if is_last_step else available_tools

                if steps == 1 and tool_policy is not None and not is_last_step:
                    missing_tools = missing_required_tools(tool_policy, available_tool_names)
                    if missing_tools:
                        error_message = format_missing_tools_error(tool_policy, missing_tools)
                        self._mark_observed_error(
                            None,
                            message=error_message,
                            error_kind="missing_required_tools",
                            step_index=steps,
                        )
                        yield {"type": "error", "error": error_message}  # type: ignore[misc]
                        return

                prepared, budget_error, context_budget_config = await self._prepare_messages_for_model(tools=requested_tools)
                if budget_error is not None:
                    self._mark_observed_error(
                        None,
                        message=budget_error,
                        error_kind="context_budget_error",
                        step_index=steps,
                    )
                    yield {"type": "error", "error": budget_error}  # type: ignore[misc]
                    return
                assert prepared is not None
                assert context_budget_config is not None

                messages = prepared.messages
                tools = list(prepared.tools)
                max_output_tokens = prepared.max_output_tokens
                if followup_messages:
                    messages = [*messages, *followup_messages]
                    followup_messages = []
                messages = self._append_runtime_context(messages)
                if is_last_step:
                    messages = self._messages_for_final_step(messages)
                    tools = []
                fallback_stage = prepared.budget.fallback_stage if prepared.budget is not None else "budget_disabled"
                self._record_context_pack_diagnostics(
                    messages=messages,
                    fallback_stage=fallback_stage,
                    step_index=steps,
                )

                attempt = 0
                assistant_text_chunks: list[str] = []
                current_step_missing_capabilities = (
                    actionable_missing_capabilities(
                        tool_policy,
                        policy_successful_tools,
                        available_tools=available_tool_names,
                        failed_tools=policy_failed_tools,
                    )
                    if tool_policy is not None and not is_last_step
                    else []
                )
                policy_guard_active = (
                    steps == 1
                    and tool_policy is not None
                    and not is_last_step
                    and bool(current_step_missing_capabilities)
                )
                policy_retry_used = False
                while True:
                    attempt += 1
                    yielded = False
                    adapter = self.agent.adapter()
                    effective_system_prompt = self.agent.system_prompt
                    if policy_guard_active and policy_retry_used:
                        effective_system_prompt = (
                            f"{effective_system_prompt}\n\n"
                            f"{format_tool_policy_reminder(tool_policy, current_step_missing_capabilities)}"
                        )

                    recorder = self.observation_recorder
                    model_span_cm = (
                        recorder.span(
                            "model.call",
                            kind="model",
                            attributes={
                                "step_index": steps,
                                "attempt_index": attempt,
                                "tool_schema_count": len(tools),
                                "message_count": len(messages),
                                "max_output_tokens": max_output_tokens,
                            },
                        )
                        if recorder is not None
                        else None
                    )
                    buffered_events: list[StreamEvent] | None = [] if policy_guard_active else None
                    try:
                        if model_span_cm is None:
                            stream = adapter.reply_stream(
                                system=effective_system_prompt,
                                messages=messages,
                                tools=tools,
                                max_output_tokens=max_output_tokens,
                            )
                            assistant_text_chunks = []
                            async for event in stream:
                                yielded = True
                                if event["type"] == "text-delta":
                                    assistant_text_chunks.append(event["text"])
                                if buffered_events is not None:
                                    buffered_events.append(event)
                                else:
                                    yield event
                            info = await stream.info()
                        else:
                            with model_span_cm as model_span:
                                stream = adapter.reply_stream(
                                    system=effective_system_prompt,
                                    messages=messages,
                                    tools=tools,
                                    max_output_tokens=max_output_tokens,
                                )
                                assistant_text_chunks = []
                                async for event in stream:
                                    yielded = True
                                    if event["type"] == "text-delta":
                                        assistant_text_chunks.append(event["text"])
                                    if buffered_events is not None:
                                        buffered_events.append(event)
                                    else:
                                        yield event
                                info = await stream.info()
                                model_span.set_attributes(
                                    {
                                        "finish_reason": info.finish_reason,
                                        "tool_call_count": len(info.tool_calls),
                                        "input_tokens": info.usage.input_tokens,
                                        "output_tokens": info.usage.output_tokens,
                                        "cost": info.usage.cost,
                                    }
                                )

                        if policy_guard_active and tool_policy is not None:
                            assistant_content = "".join(assistant_text_chunks)
                            tool_call_names = [call.name for call in info.tool_calls]
                            if (
                                should_accept_tool_calls(tool_policy, tool_call_names)
                                or "question" in tool_call_names
                                or looks_like_clarification_request(assistant_content)
                            ):
                                assert buffered_events is not None
                                for event in buffered_events:
                                    yield event
                                policy_guard_active = False
                                break
                            if not policy_retry_used:
                                policy_retry_used = True
                                attempt = 0
                                self._log_runtime(
                                    "WARNING",
                                    "Model response missed required tool policy; retrying with reminder",
                                    category="policy",
                                    attributes={
                                        "step_index": steps,
                                        "attempt_index": attempt + 1,
                                        "scenario": getattr(tool_policy, "scenario", None),
                                    },
                                )
                                continue
                            error_message = format_tool_policy_retry_error(tool_policy, current_step_missing_capabilities)
                            self._mark_observed_error(
                                None,
                                message=error_message,
                                error_kind="tool_policy_retry_failed",
                                step_index=steps,
                                attempt_index=attempt,
                            )
                            yield {"type": "error", "error": error_message}  # type: ignore[misc]
                            return

                        break
                    except Exception as error:  # noqa: BLE001
                        if attempt > self.config.max_retry or yielded:
                            self._mark_observed_error(
                                None,
                                message=str(error),
                                error_kind=type(error).__name__,
                                step_index=steps,
                                attempt_index=attempt,
                            )
                            yield {"type": "error", "error": str(error)}  # type: ignore[misc]
                            return
                        self._log_runtime(
                            "WARNING",
                            "Model call failed; retrying",
                            category="model",
                            attributes={
                                "step_index": steps,
                                "attempt_index": attempt,
                                "error_kind": type(error).__name__,
                                "message": str(error),
                            },
                        )
                        await asyncio.sleep(self.config.retry_base_delay_s * (2 ** (attempt - 1)))

                info_tool_calls = [] if is_last_step else info.tool_calls
                if prepared.overflow_text_only:
                    info_tool_calls = []
                assistant_content = "".join(assistant_text_chunks)
                if assistant_content or info_tool_calls:
                    metadata: dict[str, Any] = {}
                    if info_tool_calls:
                        metadata["tool_calls"] = [
                            {
                                "id": call.call_id,
                                "type": "function",
                                "function": {"name": call.name, "arguments": json.dumps(call.input, ensure_ascii=False)},
                            }
                            for call in info_tool_calls
                        ]
                    self.session.add(ChatMessage(role="assistant", content=assistant_content, metadata=metadata))

                step_failures: list[ToolFailureHint] = []
                for call in info_tool_calls:
                    if self.doom_loop_detector.record(call):
                        error_message = self._repeated_tool_call_error(call)
                        self._record_observation(
                            "doom_loop.detected",
                            kind="error",
                            status="error",
                            attributes={
                                "step_index": steps,
                                "tool_name": call.name,
                                "call_id": call.call_id,
                                "error_kind": "doom_loop",
                            },
                        )
                        self._log_runtime(
                            "ERROR",
                            "Doom loop detected",
                            category="tool",
                            attributes={
                                "step_index": steps,
                                "tool_name": call.name,
                                "call_id": call.call_id,
                                "error_kind": "doom_loop",
                            },
                        )
                        self._mark_observed_error(
                            None,
                            message=error_message,
                            error_kind="doom_loop",
                            step_index=steps,
                            attempt_index=attempt,
                        )
                        yield {"type": "error", "error": error_message}  # type: ignore[misc]
                        return

                    question_task: asyncio.Task[QuestionRequest] | None = None
                    try:
                        tool_task = asyncio.create_task(
                            self.toolkit.execute(
                                name=call.name,
                                input=call.input,
                                call_id=call.call_id,
                                context=self._tool_context(),
                            )
                        )
                        question_task = asyncio.create_task(self.question_manager.next_request())
                        while True:
                            done, _ = await asyncio.wait({tool_task, question_task}, return_when=asyncio.FIRST_COMPLETED)
                            if question_task in done:
                                request = question_task.result()
                                self._record_observation(
                                    "question.requested",
                                    kind="question",
                                    attributes={
                                        "step_index": steps,
                                        "tool_call_id": request.tool_call_id,
                                        "request_id": request.request_id,
                                        "question_count": len(request.questions),
                                    },
                                )
                                self._log_runtime(
                                    "INFO",
                                    "Question requested",
                                    category="question",
                                    attributes={
                                        "step_index": steps,
                                        "tool_call_id": request.tool_call_id,
                                        "request_id": request.request_id,
                                        "question_count": len(request.questions),
                                    },
                                )
                                yield self._question_request_event(request)  # type: ignore[misc]
                                question_task = asyncio.create_task(self.question_manager.next_request())
                                continue

                            result = await tool_task
                            break
                    except PermissionDeniedError as error:
                        result = self._build_tool_error_result(call=call, error=str(error), kind="permission_denied")
                    except PermissionAskRequiredError as error:
                        result = self._build_tool_error_result(call=call, error=str(error), kind="tool_error")
                    except Exception as error:  # noqa: BLE001
                        result = self._build_tool_error_result(call=call, error=str(error), kind="tool_error")
                    finally:
                        if question_task is not None:
                            question_task.cancel()
                            with suppress(asyncio.CancelledError):
                                await question_task

                    result = self._normalize_tool_result_error_kind(tool_name=call.name, result=result)
                    self._update_web_research_state(tool_name=call.name, result=result, state=web_research_state)
                    result, tool_message = project_tool_result_to_message(
                        result=result,
                        tool_name=call.name,
                        session_root=Path(self.session.directory).resolve(),
                        preview_bytes=int(context_budget_config["tool_context_preview_bytes"]),
                        preview_lines=int(context_budget_config["tool_context_preview_lines"]),
                        line_max_chars=int(context_budget_config["tool_context_line_max_chars"]),
                    )
                    yield {
                        "type": "tool-result",
                        "call_id": result.call_id,
                        "output": result.output,
                        "error": result.error,
                        "metadata": result.metadata,
                    }  # type: ignore[misc]
                    self.session.add(tool_message)
                    self._apply_tool_output_pruning(context_budget_config)
                    if tool_policy is not None and call.name in tool_policy.required_tools:
                        if result.error:
                            policy_failed_tools.add(call.name)
                        else:
                            policy_successful_tools.add(call.name)
                    if result.error:
                        step_failures.append(ToolFailureHint(kind=str(result.metadata.get("error_kind") or "tool_error"), tool_name=call.name))

                patch = self.snapshot_manager.patch(snapshot_id)
                if patch.get("files"):
                    self._record_observation(
                        "patch.detected",
                        kind="patch",
                        attributes={
                            "step_index": steps,
                            "snapshot_id": snapshot_id,
                            "hash": patch.get("hash"),
                            "file_count": len(patch.get("files") or []),
                            "files": [
                                item.get("path") or item.get("file") or item.get("name")
                                for item in (patch.get("files") or [])
                                if isinstance(item, dict)
                            ],
                        },
                    )
                    self._log_runtime(
                        "INFO",
                        "Workspace patch detected",
                        category="patch",
                        attributes={
                            "step_index": steps,
                            "snapshot_id": snapshot_id,
                            "hash": patch.get("hash"),
                            "file_count": len(patch.get("files") or []),
                        },
                    )
                    yield {"type": "patch", "snapshot_id": snapshot_id, "hash": patch["hash"], "files": patch["files"]}  # type: ignore[misc]
                usage: Usage = info.usage
                self._record_model_usage(usage)
                finish_reason: FinishReason = info.finish_reason
                if info_tool_calls and finish_reason == "unknown":
                    finish_reason = "tool_call"
                self._record_observation(
                    "step.finished",
                    kind="step",
                    attributes={
                        "step_index": steps,
                        "attempt_index": attempt,
                        "finish_reason": finish_reason,
                        "tool_call_count": len(info_tool_calls),
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens,
                        "cost": usage.cost,
                    },
                    duration_ms=int((time.time() - step_started_at) * 1000),
                )
                self._log_runtime(
                    "INFO",
                    "Agent step finished",
                    category="step",
                    attributes={
                        "step_index": steps,
                        "attempt_index": attempt,
                        "finish_reason": finish_reason,
                        "tool_call_count": len(info_tool_calls),
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens,
                        "cost": usage.cost,
                        "duration_ms": int((time.time() - step_started_at) * 1000),
                    },
                )
                yield {
                    "type": "step-finish",
                    "tokens": {"input": usage.input_tokens, "output": usage.output_tokens},
                    "cost": usage.cost,
                    "finish_reason": finish_reason,
                }  # type: ignore[misc]
                next_followup_messages: list[ChatMessage] = []
                if step_failures:
                    next_followup_messages.extend(self._failure_followup_messages(step_failures))
                web_convergence_message = self._web_research_convergence_message(web_research_state)
                if web_convergence_message is not None:
                    next_followup_messages.append(web_convergence_message)
                if (
                    tool_policy is not None
                    and finish_reason == "stop"
                    and not info_tool_calls
                    and not is_last_step
                    and not policy_soft_followup_used
                ):
                    remaining_capabilities = actionable_missing_capabilities(
                        tool_policy,
                        policy_successful_tools,
                        available_tools=available_tool_names,
                        failed_tools=policy_failed_tools,
                    )
                    if remaining_capabilities:
                        next_followup_messages.extend(
                            self._policy_followup_messages(
                                tool_policy,
                                remaining_capabilities,
                                failed_tools=policy_failed_tools,
                            )
                        )
                        policy_soft_followup_used = True
                if next_followup_messages:
                    followup_messages = next_followup_messages
                if info_tool_calls:
                    continue
                if followup_messages and finish_reason == "stop" and not is_last_step:
                    continue
                if finish_reason == "stop" or is_last_step:
                    self._record_observation(
                        "run.finished",
                        kind="run",
                        attributes={
                            "status": "completed",
                            "steps": steps,
                            "finish_reason": finish_reason,
                        },
                    )
                    self._log_runtime(
                        "INFO",
                        "Agent run finished",
                        category="run",
                        attributes={
                            "status": "completed",
                            "steps": steps,
                            "finish_reason": finish_reason,
                        },
                    )
                    return
            self._mark_observed_error(
                None,
                message="max_steps exceeded",
                error_kind="max_steps_exceeded",
                step_index=steps,
            )
            yield {"type": "error", "error": "max_steps exceeded"}  # type: ignore[misc]
        finally:
            self.session.status = SessionStatus.STOP


__all__ = ["AgentLoop", "AgentLoopConfig"]
