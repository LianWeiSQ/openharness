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
from ..context_messages import (
    CONTEXT_COMPACTION_METADATA_KEY,
    build_messages_for_model,
    build_trimmed_messages_for_model,
    count_new_messages_since_compaction,
    get_context_compaction,
    project_tool_result_to_message,
    prune_old_tool_messages,
    recent_user_turn_start,
)
from ..execution import build_workspace_runtime
from ..permission.manager import PermissionAskRequiredError, PermissionDeniedError, PermissionManager
from ..permission.ruleset import PermissionRuleset
from ..question import QuestionManager, QuestionRequest
from ..tool.middleware import logging_middleware, permission_middleware
from ..tool.toolkit import ToolkitAdapter
from ..tool_policy import (
    classify_tool_policy,
    format_missing_tools_error,
    format_tool_policy_retry_error,
    looks_like_clarification_request,
    missing_required_tools,
    should_accept_tool_calls,
)
from ..types import ChatMessage, FinishReason, SessionStatus, StreamEvent, ToolResult, ToolSchema, Usage
from ...adapter.memory_adapter import MemoryAdapter
from .doom_loop import DoomLoopDetector
from .snapshot import SnapshotManager

COMPACTION_SYSTEM_PROMPT = (
    "You condense coding sessions so a later model call can continue the work with minimal loss. "
    "Write a concise but specific summary."
)
COMPACTION_USER_PROMPT = (
    "Summarize the conversation above for continuation in a new context. Include: the current goal; completed work; "
    "important tool findings; key files read or changed; the current todo list; blockers or open questions; and the "
    "most likely next step. Keep it compact but concrete."
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
            allowed_names = {"read", "glob", "grep", "ls", "todoread", "question"}
            return [tool for tool in tools if tool.name in allowed_names]
        if isinstance(allow, list):
            allowed_names = set(allow)
            return [tool for tool in tools if tool.name in allowed_names]
        return tools

    def _init_tools(self) -> None:
        self.toolkit.register_middleware(permission_middleware(self.permission_manager))
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
        self.session.metadata["last_context_budget"] = {
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
        }

    def _record_model_usage(self, usage: Usage) -> None:
        self.session.metadata["last_model_usage"] = {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "cost": usage.cost,
        }
        self.session.metadata["last_model_usage_at"] = int(time.time() * 1000)

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
        self.session.metadata["last_tool_output_prune_tokens"] = reclaimed
        return reclaimed

    async def _run_compaction_summary(self, messages_to_compact: list[ChatMessage], *, max_output_tokens: int) -> str:
        todo_payload = [todo.to_dict() for todo in self.session.todos]
        todo_section = ""
        if todo_payload:
            todo_section = "\nCurrent todo list:\n" + json.dumps(todo_payload, ensure_ascii=False)

        prompt_messages = [
            *self._append_runtime_context(messages_to_compact),
            ChatMessage(role="user", content=COMPACTION_USER_PROMPT + todo_section),
        ]
        chunks: list[str] = []
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

    async def _compact_context(self, config: dict[str, Any]) -> bool:
        compacted_until = recent_user_turn_start(self.session.messages, int(config["prune_keep_recent_user_turns"]))
        if compacted_until <= 0:
            return False

        messages_to_compact = list(self.session.messages[:compacted_until])
        if not messages_to_compact:
            return False

        summary = await self._run_compaction_summary(
            messages_to_compact,
            max_output_tokens=int(config["compact_summary_max_output_tokens"]),
        )
        if not summary:
            return False

        self.session.metadata[CONTEXT_COMPACTION_METADATA_KEY] = {
            "summary": summary,
            "compacted_until": compacted_until,
            "updated_at": int(time.time() * 1000),
        }
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
        if "tool_error" in kinds:
            lines.append(
                "A tool execution failed. Prefer summarizing the failure and trying a different source or method instead of blindly retrying the same call."
            )
        if tool_names:
            lines.append("Failed tools: " + ", ".join(tool_names))
        return [ChatMessage(role="assistant", content="\n".join(lines), metadata={"synthetic": True, "tool_failure_followup": True})]

    async def run(self, user_text: str):
        self.permission_manager.set_ruleset(PermissionRuleset[self.agent.config.permission])
        self.session.status = SessionStatus.RUNNING
        self.session.add(ChatMessage(role="user", content=user_text))
        tool_policy = classify_tool_policy(user_text) if self.agent.uses_default_system_prompt else None
        followup_messages: list[ChatMessage] = []

        steps = 0
        try:
            while steps < self.config.max_steps:
                steps += 1
                is_last_step = steps >= self.config.max_steps
                snapshot_id = self.snapshot_manager.track(Path(self.session.directory))
                yield {"type": "step-start", "snapshot_id": snapshot_id}  # type: ignore[misc]
                available_tools = self._tools_for_agent()
                requested_tools = [] if is_last_step else available_tools

                if steps == 1 and tool_policy is not None and not is_last_step:
                    missing_tools = missing_required_tools(tool_policy, {tool.name for tool in available_tools})
                    if missing_tools:
                        yield {"type": "error", "error": format_missing_tools_error(tool_policy, missing_tools)}  # type: ignore[misc]
                        return

                prepared, budget_error, context_budget_config = await self._prepare_messages_for_model(tools=requested_tools)
                if budget_error is not None:
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

                attempt = 0
                assistant_text_chunks: list[str] = []
                policy_guard_active = steps == 1 and tool_policy is not None and not is_last_step
                policy_retry_used = False
                while True:
                    attempt += 1
                    yielded = False
                    adapter = self.agent.adapter()
                    effective_system_prompt = self.agent.system_prompt
                    if policy_guard_active and policy_retry_used:
                        effective_system_prompt = f"{effective_system_prompt}\n\n{tool_policy.reminder}"

                    stream = adapter.reply_stream(
                        system=effective_system_prompt,
                        messages=messages,
                        tools=tools,
                        max_output_tokens=max_output_tokens,
                    )
                    buffered_events: list[StreamEvent] | None = [] if policy_guard_active else None
                    try:
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

                        if policy_guard_active and tool_policy is not None:
                            assistant_content = "".join(assistant_text_chunks)
                            tool_call_names = [call.name for call in info.tool_calls]
                            if should_accept_tool_calls(tool_policy, tool_call_names) or looks_like_clarification_request(assistant_content):
                                assert buffered_events is not None
                                for event in buffered_events:
                                    yield event
                                policy_guard_active = False
                                break
                            if not policy_retry_used:
                                policy_retry_used = True
                                attempt = 0
                                continue
                            yield {"type": "error", "error": format_tool_policy_retry_error(tool_policy)}  # type: ignore[misc]
                            return

                        break
                    except Exception as error:  # noqa: BLE001
                        if attempt > self.config.max_retry or yielded:
                            yield {"type": "error", "error": str(error)}  # type: ignore[misc]
                            return
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
                        yield {"type": "error", "error": self._repeated_tool_call_error(call)}  # type: ignore[misc]
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
                    if result.error:
                        step_failures.append(ToolFailureHint(kind=str(result.metadata.get("error_kind") or "tool_error"), tool_name=call.name))

                patch = self.snapshot_manager.patch(snapshot_id)
                if patch.get("files"):
                    yield {"type": "patch", "snapshot_id": snapshot_id, "hash": patch["hash"], "files": patch["files"]}  # type: ignore[misc]
                usage: Usage = info.usage
                self._record_model_usage(usage)
                finish_reason: FinishReason = info.finish_reason
                if info_tool_calls and finish_reason == "unknown":
                    finish_reason = "tool_call"
                yield {
                    "type": "step-finish",
                    "tokens": {"input": usage.input_tokens, "output": usage.output_tokens},
                    "cost": usage.cost,
                    "finish_reason": finish_reason,
                }  # type: ignore[misc]
                if step_failures:
                    followup_messages = self._failure_followup_messages(step_failures)
                if info_tool_calls:
                    continue
                if finish_reason == "stop" or is_last_step:
                    return
            yield {"type": "error", "error": "max_steps exceeded"}  # type: ignore[misc]
        finally:
            self.session.status = SessionStatus.STOP


__all__ = ["AgentLoop", "AgentLoopConfig"]
