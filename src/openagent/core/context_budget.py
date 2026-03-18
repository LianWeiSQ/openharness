from __future__ import annotations

import json
import math
from dataclasses import dataclass
from typing import Any

from .types import ChatMessage, Model, ToolSchema

DEFAULT_BYTES_PER_TOKEN = 3
DEFAULT_GUARD_RATIO = 0.9
DEFAULT_TOOL_DISPLAY_MAX_BYTES = 50 * 1024
DEFAULT_TOOL_CONTEXT_PREVIEW_BYTES = 4096
DEFAULT_TOOL_CONTEXT_PREVIEW_LINES = 40
DEFAULT_TOOL_CONTEXT_LINE_MAX_CHARS = 240
DEFAULT_PRUNE_OLD_TOOL_OUTPUTS = True
DEFAULT_PRUNE_KEEP_RECENT_USER_TURNS = 2
DEFAULT_PRUNE_PROTECT_INPUT_TOKENS = 12_000
DEFAULT_PRUNE_MIN_INPUT_TOKENS = 4_000
DEFAULT_COMPACT_SUMMARY_MAX_OUTPUT_TOKENS = 512
DEFAULT_COMPACT_REFRESH_MIN_NEW_MESSAGES = 6
SUPPORTED_STRATEGIES = {"error", "compact"}


@dataclass(frozen=True, slots=True)
class ContextBudgetResult:
    estimated_input_tokens: int
    input_limit_tokens: int
    context_window: int
    reserved_output_tokens: int
    overflowed: bool
    tool_message_count: int = 0
    largest_tool_message_tokens: int = 0
    largest_tool_message_name: str = ""


class ContextBudgetConfigError(ValueError):
    """Raised when context budget options are invalid or unsupported."""


def check_context_budget(
    *,
    system: str | None,
    messages: list[ChatMessage],
    tools: list[ToolSchema],
    model: Model | None,
    options: dict[str, Any] | None = None,
) -> ContextBudgetResult | None:
    """
    Estimate prompt size using a conservative local heuristic.

    Returns None when the guard is disabled or model metadata is unavailable.
    """

    if model is None or model.context_window <= 0:
        return None

    config = load_context_budget_options(options, model=model)
    if not config["enabled"]:
        return None

    strategy = config["strategy"]
    if strategy not in SUPPORTED_STRATEGIES:
        raise ContextBudgetConfigError(
            f"Unsupported context budget strategy: {strategy}. "
            "Supported strategies: error, compact. "
            "TODO: trim is reserved for a future implementation."
        )

    reserved_output_tokens = int(config["reserve_output_tokens"])
    input_limit_tokens = max(int(model.context_window * float(config["guard_ratio"])) - reserved_output_tokens, 0)
    estimated_input_tokens = _estimate_input_tokens(
        system=system,
        messages=messages,
        tools=tools,
        model=model,
        bytes_per_token=int(config["bytes_per_token"]),
    )
    diagnostics = _tool_message_diagnostics(messages=messages, bytes_per_token=int(config["bytes_per_token"]))
    return ContextBudgetResult(
        estimated_input_tokens=estimated_input_tokens,
        input_limit_tokens=input_limit_tokens,
        context_window=model.context_window,
        reserved_output_tokens=reserved_output_tokens,
        overflowed=estimated_input_tokens > input_limit_tokens,
        tool_message_count=diagnostics["tool_message_count"],
        largest_tool_message_tokens=diagnostics["largest_tool_message_tokens"],
        largest_tool_message_name=diagnostics["largest_tool_message_name"],
    )


def format_context_budget_error(result: ContextBudgetResult) -> str:
    message = (
        "Context budget exceeded before model call: "
        f"estimated_input_tokens={result.estimated_input_tokens}, "
        f"input_limit_tokens={result.input_limit_tokens}, "
        f"context_window={result.context_window}, "
        f"reserved_output_tokens={result.reserved_output_tokens}"
    )
    if result.tool_message_count > 0:
        message += (
            f", tool_message_count={result.tool_message_count}, "
            f"largest_tool_message_tokens={result.largest_tool_message_tokens}, "
            f"largest_tool_message_name={result.largest_tool_message_name or 'unknown'}"
        )
    return message


def load_context_budget_options(
    options: dict[str, Any] | None,
    *,
    model: Model | None,
) -> dict[str, Any]:
    raw_options = options or {}
    raw_context_budget = raw_options.get("context_budget", {})
    if not isinstance(raw_context_budget, dict):
        raise ContextBudgetConfigError("AgentConfig.options['context_budget'] must be a dict.")

    enabled = _expect_bool(raw_context_budget.get("enabled", True), field_name="enabled")
    strategy = raw_context_budget.get("strategy", "error")
    if not isinstance(strategy, str) or not strategy.strip():
        raise ContextBudgetConfigError("context_budget.strategy must be a non-empty string.")

    reserve_output_tokens = raw_context_budget.get("reserve_output_tokens", model.max_output if model is not None else 0)
    guard_ratio = raw_context_budget.get("guard_ratio", DEFAULT_GUARD_RATIO)
    bytes_per_token = raw_context_budget.get("bytes_per_token", DEFAULT_BYTES_PER_TOKEN)
    tool_display_max_bytes = raw_context_budget.get("tool_display_max_bytes", DEFAULT_TOOL_DISPLAY_MAX_BYTES)
    tool_context_preview_bytes = raw_context_budget.get("tool_context_preview_bytes", DEFAULT_TOOL_CONTEXT_PREVIEW_BYTES)
    tool_context_preview_lines = raw_context_budget.get("tool_context_preview_lines", DEFAULT_TOOL_CONTEXT_PREVIEW_LINES)
    tool_context_line_max_chars = raw_context_budget.get("tool_context_line_max_chars", DEFAULT_TOOL_CONTEXT_LINE_MAX_CHARS)
    prune_old_tool_outputs = raw_context_budget.get("prune_old_tool_outputs", DEFAULT_PRUNE_OLD_TOOL_OUTPUTS)
    prune_keep_recent_user_turns = raw_context_budget.get("prune_keep_recent_user_turns", DEFAULT_PRUNE_KEEP_RECENT_USER_TURNS)
    prune_protect_input_tokens = raw_context_budget.get("prune_protect_input_tokens", DEFAULT_PRUNE_PROTECT_INPUT_TOKENS)
    prune_min_input_tokens = raw_context_budget.get("prune_min_input_tokens", DEFAULT_PRUNE_MIN_INPUT_TOKENS)
    compact_summary_max_output_tokens = raw_context_budget.get(
        "compact_summary_max_output_tokens",
        DEFAULT_COMPACT_SUMMARY_MAX_OUTPUT_TOKENS,
    )
    compact_refresh_min_new_messages = raw_context_budget.get(
        "compact_refresh_min_new_messages",
        DEFAULT_COMPACT_REFRESH_MIN_NEW_MESSAGES,
    )

    reserve_output_tokens = _expect_int(
        reserve_output_tokens,
        field_name="reserve_output_tokens",
        minimum=0,
    )
    guard_ratio = _expect_float(
        guard_ratio,
        field_name="guard_ratio",
        minimum=0.0,
        maximum=1.0,
        include_minimum=False,
    )
    bytes_per_token = _expect_int(bytes_per_token, field_name="bytes_per_token", minimum=1)
    tool_display_max_bytes = _expect_int(tool_display_max_bytes, field_name="tool_display_max_bytes", minimum=1)
    tool_context_preview_bytes = _expect_int(
        tool_context_preview_bytes,
        field_name="tool_context_preview_bytes",
        minimum=1,
    )
    tool_context_preview_lines = _expect_int(
        tool_context_preview_lines,
        field_name="tool_context_preview_lines",
        minimum=1,
    )
    tool_context_line_max_chars = _expect_int(
        tool_context_line_max_chars,
        field_name="tool_context_line_max_chars",
        minimum=1,
    )
    prune_old_tool_outputs = _expect_bool(prune_old_tool_outputs, field_name="prune_old_tool_outputs")
    prune_keep_recent_user_turns = _expect_int(
        prune_keep_recent_user_turns,
        field_name="prune_keep_recent_user_turns",
        minimum=1,
    )
    prune_protect_input_tokens = _expect_int(
        prune_protect_input_tokens,
        field_name="prune_protect_input_tokens",
        minimum=0,
    )
    prune_min_input_tokens = _expect_int(
        prune_min_input_tokens,
        field_name="prune_min_input_tokens",
        minimum=0,
    )
    compact_summary_max_output_tokens = _expect_int(
        compact_summary_max_output_tokens,
        field_name="compact_summary_max_output_tokens",
        minimum=1,
    )
    compact_refresh_min_new_messages = _expect_int(
        compact_refresh_min_new_messages,
        field_name="compact_refresh_min_new_messages",
        minimum=1,
    )
    return {
        "enabled": enabled,
        "strategy": strategy.strip(),
        "reserve_output_tokens": reserve_output_tokens,
        "guard_ratio": guard_ratio,
        "bytes_per_token": bytes_per_token,
        "tool_display_max_bytes": tool_display_max_bytes,
        "tool_context_preview_bytes": tool_context_preview_bytes,
        "tool_context_preview_lines": tool_context_preview_lines,
        "tool_context_line_max_chars": tool_context_line_max_chars,
        "prune_old_tool_outputs": prune_old_tool_outputs,
        "prune_keep_recent_user_turns": prune_keep_recent_user_turns,
        "prune_protect_input_tokens": prune_protect_input_tokens,
        "prune_min_input_tokens": prune_min_input_tokens,
        "compact_summary_max_output_tokens": compact_summary_max_output_tokens,
        "compact_refresh_min_new_messages": compact_refresh_min_new_messages,
    }


def _estimate_input_tokens(
    *,
    system: str | None,
    messages: list[ChatMessage],
    tools: list[ToolSchema],
    model: Model,
    bytes_per_token: int,
) -> int:
    payload = {
        "model": model.id,
        "messages": _normalize_messages(system=system, messages=messages),
        "tools": _normalize_tools(tools),
    }
    serialized = json.dumps(payload, ensure_ascii=False, separators=(",", ":"), default=str)
    return max(1, math.ceil(len(serialized.encode("utf-8")) / bytes_per_token))


def estimate_message_tokens(message: ChatMessage, *, bytes_per_token: int = DEFAULT_BYTES_PER_TOKEN) -> int:
    serialized = json.dumps(
        _normalize_messages(system=None, messages=[message]),
        ensure_ascii=False,
        separators=(",", ":"),
        default=str,
    )
    return max(1, math.ceil(len(serialized.encode("utf-8")) / bytes_per_token))


def _normalize_messages(*, system: str | None, messages: list[ChatMessage]) -> list[dict[str, Any]]:
    normalized: list[dict[str, Any]] = []
    if system:
        normalized.append({"role": "system", "content": system})
    for message in messages:
        item: dict[str, Any] = {"role": message.role, "content": message.content}
        if message.role != "tool" and message.name:
            item["name"] = message.name
        if message.tool_call_id:
            item["tool_call_id"] = message.tool_call_id
        tool_calls = (message.metadata or {}).get("tool_calls")
        if message.role == "assistant" and isinstance(tool_calls, list) and tool_calls:
            item["tool_calls"] = tool_calls
            if not message.content:
                item["content"] = None
        normalized.append(item)
    return normalized


def _normalize_tools(tools: list[ToolSchema]) -> list[dict[str, Any]]:
    normalized: list[dict[str, Any]] = []
    for tool in tools:
        normalized.append(
            {
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.schema or {"type": "object", "properties": {}},
                },
            }
        )
    return normalized


def _tool_message_diagnostics(*, messages: list[ChatMessage], bytes_per_token: int) -> dict[str, Any]:
    tool_message_count = 0
    largest_tool_message_tokens = 0
    largest_tool_message_name = ""
    for message in messages:
        if message.role != "tool":
            continue
        tool_message_count += 1
        estimate = estimate_message_tokens(message, bytes_per_token=bytes_per_token)
        if estimate > largest_tool_message_tokens:
            largest_tool_message_tokens = estimate
            largest_tool_message_name = message.name or ""
    return {
        "tool_message_count": tool_message_count,
        "largest_tool_message_tokens": largest_tool_message_tokens,
        "largest_tool_message_name": largest_tool_message_name,
    }


# TODO: support strategy="trim" by applying a sliding window plus structured summary.
# TODO: plug in provider-specific tokenizers when precision becomes more important than portability.


def _expect_bool(value: Any, *, field_name: str) -> bool:
    if isinstance(value, bool):
        return value
    raise ContextBudgetConfigError(f"context_budget.{field_name} must be a bool.")


def _expect_int(value: Any, *, field_name: str, minimum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ContextBudgetConfigError(f"context_budget.{field_name} must be an int.")
    if value < minimum:
        raise ContextBudgetConfigError(f"context_budget.{field_name} must be >= {minimum}.")
    return value


def _expect_float(
    value: Any,
    *,
    field_name: str,
    minimum: float,
    maximum: float,
    include_minimum: bool = True,
) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ContextBudgetConfigError(f"context_budget.{field_name} must be a number.")
    number = float(value)
    if include_minimum:
        if number < minimum:
            raise ContextBudgetConfigError(f"context_budget.{field_name} must be >= {minimum}.")
    elif number <= minimum:
        raise ContextBudgetConfigError(f"context_budget.{field_name} must be > {minimum}.")
    if number > maximum:
        raise ContextBudgetConfigError(f"context_budget.{field_name} must be <= {maximum}.")
    return number
