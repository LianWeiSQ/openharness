from __future__ import annotations

import os
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Literal

from .context_budget import ContextBudgetResult
from .types import Usage

RuntimeWarningSeverity = Literal["info", "warning", "critical"]
_ENV_OPTION_MAP = {
    "RUNTIME_WARNINGS_ENABLED": "enabled",
    "CONTEXT_WARNING_RATIO": "context_usage_ratio",
    "CONTEXT_CRITICAL_RATIO": "context_critical_ratio",
    "MAX_STEP_INPUT_TOKENS": "max_step_input_tokens",
    "MAX_STEP_OUTPUT_TOKENS": "max_step_output_tokens",
    "MAX_STEP_TOTAL_TOKENS": "max_step_total_tokens",
    "MAX_STEP_COST": "max_step_cost",
}


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
            "display": _display_payload(self.code, self.severity, self.message, self.metrics),
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


def runtime_warning_options_from_env(
    environ: Mapping[str, str] | None = None,
    *,
    prefixes: Sequence[str] = ("OPENAGENT",),
) -> dict[str, Any]:
    env = environ or os.environ
    options: dict[str, Any] = {}
    for suffix, option_key in _ENV_OPTION_MAP.items():
        for prefix in prefixes:
            name = f"{prefix}_{suffix}" if prefix else suffix
            if name in env:
                options[option_key] = env[name]
                break
    return options


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


def format_runtime_warning_event(event: dict[str, Any]) -> str | None:
    if event.get("type") != "runtime-warning":
        return None
    display = event.get("display") if isinstance(event.get("display"), dict) else {}
    severity = str(display.get("severity") or event.get("severity") or "warning").upper()
    title = str(display.get("title") or event.get("code") or "Runtime warning")
    body = str(display.get("body") or event.get("message") or "").strip()
    metrics = display.get("metrics")
    metric_text = _format_display_metrics(metrics if isinstance(metrics, dict) else {})
    suffix = f" ({metric_text})" if metric_text else ""
    return f"[{severity}] {title}: {body}{suffix}"


def _display_payload(code: str, severity: RuntimeWarningSeverity, message: str, metrics: dict[str, Any]) -> dict[str, Any]:
    return {
        "kind": "runtime_warning",
        "severity": severity,
        "title": _warning_title(code),
        "body": message,
        "metrics": _display_metrics(code, metrics),
    }


def _warning_title(code: str) -> str:
    titles = {
        "context_usage_high": "Context usage high",
        "context_usage_critical": "Context usage critical",
        "step_input_tokens_exceeded": "Step input token budget exceeded",
        "step_output_tokens_exceeded": "Step output token budget exceeded",
        "step_total_tokens_exceeded": "Step token budget exceeded",
        "step_cost_exceeded": "Step cost budget exceeded",
    }
    return titles.get(code, code.replace("_", " ").title())


def _display_metrics(code: str, metrics: dict[str, Any]) -> dict[str, Any]:
    if code.startswith("context_usage_"):
        return _compact_metrics(
            metrics,
            [
                "step_index",
                "usage_ratio",
                "threshold",
                "estimated_input_tokens",
                "input_limit_tokens",
                "fallback_stage",
            ],
        )
    if code == "step_cost_exceeded":
        return _compact_metrics(metrics, ["step_index", "cost", "threshold", "input_tokens", "output_tokens", "total_tokens"])
    if code.startswith("step_"):
        return _compact_metrics(metrics, ["step_index", "input_tokens", "output_tokens", "total_tokens", "threshold"])
    return _compact_metrics(metrics, sorted(metrics))


def _compact_metrics(metrics: dict[str, Any], keys: list[str]) -> dict[str, Any]:
    compact: dict[str, Any] = {}
    for key in keys:
        if key not in metrics or metrics[key] is None:
            continue
        compact[key] = metrics[key]
    return compact


def _format_display_metrics(metrics: dict[str, Any]) -> str:
    parts: list[str] = []
    for key in sorted(metrics):
        value = metrics[key]
        if isinstance(value, float):
            if key.endswith("ratio") or key == "threshold" and 0 < value <= 1:
                text = f"{value:.1%}"
            else:
                text = f"{value:.6f}".rstrip("0").rstrip(".")
        else:
            text = str(value)
        parts.append(f"{key}={text}")
    return ", ".join(parts)


__all__ = [
    "RuntimeWarningConfig",
    "RuntimeWarningRecord",
    "RuntimeWarningSeverity",
    "context_budget_warning",
    "format_runtime_warning_event",
    "load_runtime_warning_config",
    "runtime_warning_options_from_env",
    "step_usage_warnings",
]
