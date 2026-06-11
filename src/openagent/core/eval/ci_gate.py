from __future__ import annotations

import argparse
import json
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Sequence


@dataclass(frozen=True, slots=True)
class EvalCiGateResult:
    ok: bool
    status: str
    reasons: list[str] = field(default_factory=list)
    metrics: dict[str, Any] = field(default_factory=dict)
    report_path: str | None = None
    regression_path: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


def check_eval_ci_gate(
    report_path: str | Path,
    *,
    regression_path: str | Path | None = None,
    min_success_rate: float = 1.0,
    max_runtime_warnings: int | None = None,
    require_trace_check: bool = True,
    fail_on_budget_regressions: bool = True,
    fail_on_status_regressions: bool = True,
) -> EvalCiGateResult:
    report_file = Path(report_path)
    report = _read_json(report_file)
    aggregate = report.get("aggregate") if isinstance(report.get("aggregate"), dict) else {}
    results = report.get("results") if isinstance(report.get("results"), list) else []
    success_rate = _float_metric(aggregate, "success_rate", default=_computed_success_rate(results))
    trace_check_failed = _int_metric(aggregate, "trace_check_failed", default=_count_trace_check_failed(results))
    runtime_warning_count = _int_metric(aggregate, "runtime_warning_count", default=_sum_result_int(results, "runtime_warning_count"))
    regression_file = Path(regression_path) if regression_path is not None else None
    regression_summary = _load_regression_summary(regression_file=regression_file, report=report)

    reasons: list[str] = []
    if success_rate < min_success_rate:
        reasons.append(f"success_rate below min_success_rate: {success_rate:.3f} < {min_success_rate:.3f}")
    if require_trace_check and trace_check_failed > 0:
        reasons.append(f"trace_check_failed must be 0: {trace_check_failed}")
    if max_runtime_warnings is not None and runtime_warning_count > max_runtime_warnings:
        reasons.append(f"runtime_warning_count exceeded max_runtime_warnings: {runtime_warning_count} > {max_runtime_warnings}")
    if fail_on_status_regressions and int(regression_summary.get("status_regressions") or 0) > 0:
        reasons.append(f"status_regressions must be 0: {int(regression_summary.get('status_regressions') or 0)}")
    if fail_on_budget_regressions and int(regression_summary.get("budget_regressions") or 0) > 0:
        reasons.append(f"budget_regressions must be 0: {int(regression_summary.get('budget_regressions') or 0)}")

    metrics = {
        "success_rate": success_rate,
        "min_success_rate": min_success_rate,
        "trace_check_failed": trace_check_failed,
        "runtime_warning_count": runtime_warning_count,
        "max_runtime_warnings": max_runtime_warnings,
        "status_regressions": int(regression_summary.get("status_regressions") or 0),
        "budget_regressions": int(regression_summary.get("budget_regressions") or 0),
    }
    return EvalCiGateResult(
        ok=not reasons,
        status="pass" if not reasons else "fail",
        reasons=reasons,
        metrics=metrics,
        report_path=str(report_file),
        regression_path=str(regression_file) if regression_file is not None else None,
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="openagent-eval-ci-gate", description="Fail CI when an OpenAgent eval report violates release gates.")
    parser.add_argument("--report", required=True, help="Path to eval report.json.")
    parser.add_argument("--regression", help="Optional path to regression.json.")
    parser.add_argument("--min-success-rate", type=float, default=1.0, help="Minimum aggregate success rate. Defaults to 1.0.")
    parser.add_argument("--max-runtime-warnings", type=int, help="Maximum allowed aggregate runtime warnings.")
    parser.add_argument("--allow-trace-check-failures", action="store_true", help="Do not fail when trace_check_failed is non-zero.")
    parser.add_argument("--allow-budget-regressions", action="store_true", help="Do not fail when regression budget_regressions is non-zero.")
    parser.add_argument("--allow-status-regressions", action="store_true", help="Do not fail when regression status_regressions is non-zero.")
    args = parser.parse_args(list(argv) if argv is not None else None)
    result = check_eval_ci_gate(
        args.report,
        regression_path=args.regression,
        min_success_rate=args.min_success_rate,
        max_runtime_warnings=args.max_runtime_warnings,
        require_trace_check=not args.allow_trace_check_failures,
        fail_on_budget_regressions=not args.allow_budget_regressions,
        fail_on_status_regressions=not args.allow_status_regressions,
    )
    print(json.dumps(result.to_dict(), ensure_ascii=False, indent=2, sort_keys=True))
    return 0 if result.ok else 1


def _read_json(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError(f"Expected JSON object: {path}")
    return payload


def _load_regression_summary(*, regression_file: Path | None, report: dict[str, Any]) -> dict[str, Any]:
    if regression_file is not None:
        regression = _read_json(regression_file)
        summary = regression.get("summary")
        return dict(summary) if isinstance(summary, dict) else {}
    regression = report.get("regression")
    if not isinstance(regression, dict):
        return {}
    summary = regression.get("summary")
    return dict(summary) if isinstance(summary, dict) else {}


def _computed_success_rate(results: list[Any]) -> float:
    if not results:
        return 0.0
    passed = sum(1 for item in results if isinstance(item, dict) and item.get("status") == "pass")
    return passed / len(results)


def _count_trace_check_failed(results: list[Any]) -> int:
    return sum(1 for item in results if isinstance(item, dict) and not bool(item.get("trace_check_ok")))


def _sum_result_int(results: list[Any], key: str) -> int:
    total = 0
    for item in results:
        if not isinstance(item, dict):
            continue
        try:
            total += int(item.get(key) or 0)
        except (TypeError, ValueError):
            continue
    return total


def _float_metric(aggregate: dict[str, Any], key: str, *, default: float) -> float:
    try:
        return float(aggregate.get(key, default))
    except (TypeError, ValueError):
        return default


def _int_metric(aggregate: dict[str, Any], key: str, *, default: int) -> int:
    try:
        return int(aggregate.get(key, default))
    except (TypeError, ValueError):
        return default


if __name__ == "__main__":
    raise SystemExit(main())


__all__ = ["EvalCiGateResult", "check_eval_ci_gate", "main"]
