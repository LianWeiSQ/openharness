from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Literal

from .context_budget import ContextBudgetResult
from .types import Usage

RuntimeWarningSeverity = Literal["info", "warning", "critical"]


@dataclass(frozen=True, slots=True)
class RuntimeWarningConfig:
    enabled: bool = False
    context_usage_ratio: float | None = None
    context_critical_ratio: float | None = None
    max_step_input_tokens: int | None = None
    max_step_output_tokens: int | None = None
    max_step_total_tokens: int | None = None
    max_step_cost: float | None = None


@dataclass(frozen=True, slots=True)
class RuntimeWarningRecord:
    code: str
    severity: RuntimeWarningSeverity
    message: str
    metrics: dict[str, Any]

    def to_event(self) -> dict[str, Any]:
        return {
            "type": "runtime-warning",
            "severity": self.severity,
            "code": self.code,
            "message": self.message,
            "metrics": dict(self.metrics),
        }


def load_runtime_warning_config(options: dict[str, Any] | None) -> RuntimeWarningConfig:
    raw_options = options or {}
    raw = raw_options.get("runtime_warnings")
    if raw is None:
        raw = {}
    if not isinstance(raw, dict):
        raw = {}
    threshold_keys = {
        "context_usage_ratio",
        "context_critical_ratio",
        "max_step_input_tokens",
        "max_step_output_tokens",
        "max_step_total_tokens",
        "max_step_cost",
    }
    enabled = _bool_option(raw.get("enabled", any(key in raw for key in threshold_keys)))
    return RuntimeWarningConfig(
        enabled=enabled,
        context_usage_ratio=_ratio_option(raw.get("context_usage_ratio")),
        context_critical_ratio=_ratio_option(raw.get("context_critical_ratio")),
        max_step_input_tokens=_positive_int_option(raw.get("max_step_input_tokens")),
        max_step_output_tokens=_positive_int_option(raw.get("max_step_output_tokens")),
        max_step_total_tokens=_positive_int_option(raw.get("max_step_total_tokens")),
        max_step_cost=_positive_float_option(raw.get("max_step_cost")),
    )


def context_budget_warning(
    config: RuntimeWarningConfig,
    budget: ContextBudgetResult | None,
    *,
    step_index: int,
) -> RuntimeWarningRecord | None:
    if not config.enabled or budget is None or budget.input_limit_tokens <= 0:
        return None
    ratio = budget.estimated_input_tokens / budget.input_limit_tokens
    if config.context_critical_ratio is not None and ratio >= config.context_critical_ratio:
        return _context_warning(
            code="context_usage_critical",
            severity="critical",
            ratio=ratio,
            threshold=config.context_critical_ratio,
            budget=budget,
            step_index=step_index,
        )
    if config.context_usage_ratio is not None and ratio >= config.context_usage_ratio:
        return _context_warning(
            code="context_usage_high",
            severity="warning",
            ratio=ratio,
            threshold=config.context_usage_ratio,
            budget=budget,
            step_index=step_index,
        )
    return None


def step_usage_warnings(
    config: RuntimeWarningConfig,
    usage: Usage,
    *,
    step_index: int,
) -> list[RuntimeWarningRecord]:
    if not config.enabled:
        return []
    warnings: list[RuntimeWarningRecord] = []
    total_tokens = usage.input_tokens + usage.output_tokens
    base_metrics = {
        "step_index": step_index,
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "total_tokens": total_tokens,
        "cost": usage.cost,
    }
    if config.max_step_input_tokens is not None and usage.input_tokens > config.max_step_input_tokens:
        warnings.append(
            RuntimeWarningRecord(
                code="step_input_tokens_exceeded",
                severity="warning",
                message=f"Step input tokens exceeded budget: {usage.input_tokens} > {config.max_step_input_tokens}.",
                metrics={**base_metrics, "threshold": config.max_step_input_tokens},
            )
        )
    if config.max_step_output_tokens is not None and usage.output_tokens > config.max_step_output_tokens:
        warnings.append(
            RuntimeWarningRecord(
                code="step_output_tokens_exceeded",
                severity="warning",
                message=f"Step output tokens exceeded budget: {usage.output_tokens} > {config.max_step_output_tokens}.",
                metrics={**base_metrics, "threshold": config.max_step_output_tokens},
            )
        )
    if config.max_step_total_tokens is not None and total_tokens > config.max_step_total_tokens:
        warnings.append(
            RuntimeWarningRecord(
                code="step_total_tokens_exceeded",
                severity="warning",
                message=f"Step total tokens exceeded budget: {total_tokens} > {config.max_step_total_tokens}.",
                metrics={**base_metrics, "threshold": config.max_step_total_tokens},
            )
        )
    if config.max_step_cost is not None and usage.cost > config.max_step_cost:
        warnings.append(
            RuntimeWarningRecord(
                code="step_cost_exceeded",
                severity="warning",
                message=f"Step cost exceeded budget: {usage.cost:.6f} > {config.max_step_cost:.6f}.",
                metrics={**base_metrics, "threshold": config.max_step_cost},
            )
        )
    return warnings


def _context_warning(
    *,
    code: str,
    severity: RuntimeWarningSeverity,
    ratio: float,
    threshold: float,
    budget: ContextBudgetResult,
    step_index: int,
) -> RuntimeWarningRecord:
    return RuntimeWarningRecord(
        code=code,
        severity=severity,
        message=f"Context usage reached {ratio:.1%} of input budget.",
        metrics={
            "step_index": step_index,
            "usage_ratio": ratio,
            "threshold": threshold,
            "estimated_input_tokens": budget.estimated_input_tokens,
            "input_limit_tokens": budget.input_limit_tokens,
            "context_window": budget.context_window,
            "reserved_output_tokens": budget.reserved_output_tokens,
            "counting_method": budget.counting_method,
            "counting_exact": budget.counting_exact,
            "fallback_stage": budget.fallback_stage,
            "payload_kind": budget.payload_kind,
            "tool_message_count": budget.tool_message_count,
            "largest_tool_message_tokens": budget.largest_tool_message_tokens,
            "largest_tool_message_name": budget.largest_tool_message_name,
        },
    )


def _bool_option(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() not in {"0", "false", "no", "off"}
    return bool(value)


def _ratio_option(value: Any) -> float | None:
    try:
        ratio = float(value)
    except (TypeError, ValueError):
        return None
    if ratio <= 0:
        return None
    return min(ratio, 1.0)


def _positive_int_option(value: Any) -> int | None:
    try:
        number = int(value)
    except (TypeError, ValueError):
        return None
    return number if number > 0 else None


def _positive_float_option(value: Any) -> float | None:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if number > 0 else None


__all__ = [
    "RuntimeWarningConfig",
    "RuntimeWarningRecord",
    "RuntimeWarningSeverity",
    "context_budget_warning",
    "load_runtime_warning_config",
    "step_usage_warnings",
]
