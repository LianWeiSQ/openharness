from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from .message_materializer import materialize_payload
from .token_counting import count_materialized_payload
from .types import ChatMessage, Model, ToolSchema

DEFAULT_BYTES_PER_TOKEN = 3
DEFAULT_GUARD_RATIO = 0.9
DEFAULT_INPUT_SAFETY_MARGIN_TOKENS = 1024
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
DEFAULT_OVERFLOW_KEEP_RECENT_USER_TURNS = 2
SUPPORTED_STRATEGIES = {"auto", "error", "compact"}
SUPPORTED_COUNTING = {"auto", "tiktoken", "heuristic"}


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
    counting_method: str = "heuristic"
    counting_exact: bool = False
    fallback_stage: str = "initial"
    payload_kind: str = "generic"


class ContextBudgetConfigError(ValueError):
    """Raised when context budget options are invalid or unsupported."""


def check_context_budget(
    *,
    system: str | None,
    messages: list[ChatMessage],
    tools: list[ToolSchema],
    model: Model | None,
    options: dict[str, Any] | None = None,
    fallback_stage: str = "initial",
) -> ContextBudgetResult | None:
    if model is None or model.context_window <= 0:
        return None

    config = load_context_budget_options(options, model=model)
    if not config["enabled"]:
        return None

    strategy = config["strategy"]
    if strategy not in SUPPORTED_STRATEGIES:
        raise ContextBudgetConfigError(
            f"Unsupported context budget strategy: {strategy}. "
            "Supported strategies: auto, error, compact."
        )

    counting = config["counting"]
    if counting not in SUPPORTED_COUNTING:
        raise ContextBudgetConfigError(
            f"Unsupported context budget counting mode: {counting}. "
            "Supported modes: auto, tiktoken, heuristic."
        )

    reserved_output_tokens = int(config["reserve_output_tokens"])
    input_limit_tokens = _compute_input_limit_tokens(model=model, config=config)
    materialized = materialize_payload(
        system=system,
        messages=messages,
        tools=tools,
        model=model,
        options=options,
    )
    count = count_materialized_payload(
        materialized,
        model=model,
        options=options,
        counting=counting,
        bytes_per_token=int(config["bytes_per_token"]),
    )
    diagnostics = _tool_message_diagnostics(
        messages=messages,
        bytes_per_token=int(config["bytes_per_token"]),
        model=model,
        options=options,
        counting=counting,
    )
    return ContextBudgetResult(
        estimated_input_tokens=count.tokens,
        input_limit_tokens=input_limit_tokens,
        context_window=model.context_window,
        reserved_output_tokens=reserved_output_tokens,
        overflowed=count.tokens > input_limit_tokens,
        tool_message_count=diagnostics["tool_message_count"],
        largest_tool_message_tokens=diagnostics["largest_tool_message_tokens"],
        largest_tool_message_name=diagnostics["largest_tool_message_name"],
        counting_method=count.method,
        counting_exact=count.exact,
        fallback_stage=fallback_stage,
        payload_kind=materialized.payload_kind,
    )


def format_context_budget_error(result: ContextBudgetResult) -> str:
    message = (
        "Context budget exceeded before model call: "
        f"estimated_input_tokens={result.estimated_input_tokens}, "
        f"input_limit_tokens={result.input_limit_tokens}, "
        f"context_window={result.context_window}, "
        f"reserved_output_tokens={result.reserved_output_tokens}, "
        f"counting_method={result.counting_method}, "
        f"counting_exact={str(result.counting_exact).lower()}, "
        f"payload_kind={result.payload_kind}, "
        f"fallback_stage={result.fallback_stage}"
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
    strategy = raw_context_budget.get("strategy", "auto")
    if not isinstance(strategy, str) or not strategy.strip():
        raise ContextBudgetConfigError("context_budget.strategy must be a non-empty string.")

    counting = raw_context_budget.get("counting", "auto")
    if not isinstance(counting, str) or not counting.strip():
        raise ContextBudgetConfigError("context_budget.counting must be a non-empty string.")

    reserve_output_tokens = raw_context_budget.get("reserve_output_tokens", model.max_output if model is not None else 0)
    guard_ratio = raw_context_budget.get("guard_ratio", DEFAULT_GUARD_RATIO)
    explicit_input_safety_margin_tokens = "input_safety_margin_tokens" in raw_context_budget
    use_safety_margin_tokens = explicit_input_safety_margin_tokens or "guard_ratio" not in raw_context_budget
    safety_margin_default = DEFAULT_INPUT_SAFETY_MARGIN_TOKENS if use_safety_margin_tokens else 0
    input_safety_margin_tokens = raw_context_budget.get("input_safety_margin_tokens", safety_margin_default)
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
    overflow_keep_recent_user_turns = raw_context_budget.get(
        "overflow_keep_recent_user_turns",
        DEFAULT_OVERFLOW_KEEP_RECENT_USER_TURNS,
    )
    overflow_disable_tools_on_final_attempt = raw_context_budget.get("overflow_disable_tools_on_final_attempt", True)
    overflow_final_max_output_tokens = raw_context_budget.get(
        "overflow_final_max_output_tokens",
        min(512, model.max_output) if model is not None else 512,
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
    input_safety_margin_tokens = _expect_int(
        input_safety_margin_tokens,
        field_name="input_safety_margin_tokens",
        minimum=0,
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
    overflow_keep_recent_user_turns = _expect_int(
        overflow_keep_recent_user_turns,
        field_name="overflow_keep_recent_user_turns",
        minimum=1,
    )
    overflow_disable_tools_on_final_attempt = _expect_bool(
        overflow_disable_tools_on_final_attempt,
        field_name="overflow_disable_tools_on_final_attempt",
    )
    overflow_final_max_output_tokens = _expect_int(
        overflow_final_max_output_tokens,
        field_name="overflow_final_max_output_tokens",
        minimum=1,
    )
    return {
        "enabled": enabled,
        "strategy": strategy.strip(),
        "counting": counting.strip(),
        "reserve_output_tokens": reserve_output_tokens,
        "guard_ratio": guard_ratio,
        "input_safety_margin_tokens": input_safety_margin_tokens,
        "use_safety_margin_tokens": use_safety_margin_tokens,
        "explicit_input_safety_margin_tokens": explicit_input_safety_margin_tokens,
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
        "overflow_keep_recent_user_turns": overflow_keep_recent_user_turns,
        "overflow_disable_tools_on_final_attempt": overflow_disable_tools_on_final_attempt,
        "overflow_final_max_output_tokens": overflow_final_max_output_tokens,
    }


def estimate_message_tokens(
    message: ChatMessage,
    *,
    bytes_per_token: int = DEFAULT_BYTES_PER_TOKEN,
    model: Model | None = None,
    options: dict[str, Any] | None = None,
    counting: str = "heuristic",
) -> int:
    materialized = materialize_payload(
        system=None,
        messages=[message],
        tools=[],
        model=model,
        options=options,
    )
    result = count_materialized_payload(
        materialized,
        model=model,
        options=options,
        counting=counting,
        bytes_per_token=bytes_per_token,
    )
    return result.tokens


def _compute_input_limit_tokens(*, model: Model, config: dict[str, Any]) -> int:
    reserved_output_tokens = int(config["reserve_output_tokens"])
    if config["use_safety_margin_tokens"]:
        limit = model.context_window - reserved_output_tokens - int(config["input_safety_margin_tokens"])
        if limit > 0 or bool(config.get("explicit_input_safety_margin_tokens")):
            return max(limit, 0)
    return max(int(model.context_window * float(config["guard_ratio"])) - reserved_output_tokens, 0)


def _tool_message_diagnostics(
    *,
    messages: list[ChatMessage],
    bytes_per_token: int,
    model: Model | None,
    options: dict[str, Any] | None,
    counting: str,
) -> dict[str, Any]:
    tool_message_count = 0
    largest_tool_message_tokens = 0
    largest_tool_message_name = ""
    for message in messages:
        if message.role != "tool":
            continue
        tool_message_count += 1
        estimate = estimate_message_tokens(
            message,
            bytes_per_token=bytes_per_token,
            model=model,
            options=options,
            counting=counting,
        )
        if estimate > largest_tool_message_tokens:
            largest_tool_message_tokens = estimate
            largest_tool_message_name = message.name or ""
    return {
        "tool_message_count": tool_message_count,
        "largest_tool_message_tokens": largest_tool_message_tokens,
        "largest_tool_message_name": largest_tool_message_name,
    }


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
