#[must_use]
pub fn check_eval_ci_gate(
    report_path: &str,
    report: &Value,
    regression: Option<(&str, &Value)>,
    options: EvalCiGateOptions,
) -> EvalCiGateResult {
    let aggregate = report.get("aggregate").unwrap_or(&Value::Null);
    let results = report
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let success_rate = aggregate
        .get("success_rate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| computed_success_rate(&results));
    let trace_check_failed = aggregate
        .get("trace_check_failed")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| count_trace_check_failed(&results));
    let runtime_warning_count = aggregate
        .get("runtime_warning_count")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| sum_result_int(&results, "runtime_warning_count"));
    let regression_summary = regression
        .map(|(_, value)| value)
        .or_else(|| report.get("regression"))
        .and_then(|value| value.get("summary"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mut reasons = Vec::new();
    if success_rate < options.min_success_rate {
        reasons.push(format!(
            "success_rate below min_success_rate: {success_rate:.3} < {:.3}",
            options.min_success_rate
        ));
    }
    if options.require_trace_check && trace_check_failed > 0 {
        reasons.push(format!(
            "trace_check_failed must be 0: {trace_check_failed}"
        ));
    }
    if let Some(maximum) = options.max_runtime_warnings {
        if runtime_warning_count > maximum {
            reasons.push(format!(
                "runtime_warning_count exceeded max_runtime_warnings: {runtime_warning_count} > {maximum}"
            ));
        }
    }
    if options.fail_on_status_regressions
        && int_field(&regression_summary, "status_regressions") > 0
    {
        reasons.push(format!(
            "status_regressions must be 0: {}",
            int_field(&regression_summary, "status_regressions")
        ));
    }
    if options.fail_on_budget_regressions
        && int_field(&regression_summary, "budget_regressions") > 0
    {
        reasons.push(format!(
            "budget_regressions must be 0: {}",
            int_field(&regression_summary, "budget_regressions")
        ));
    }

    EvalCiGateResult {
        ok: reasons.is_empty(),
        status: if reasons.is_empty() { "pass" } else { "fail" }.to_string(),
        reasons,
        metrics: json!({
            "success_rate": success_rate,
            "min_success_rate": options.min_success_rate,
            "trace_check_failed": trace_check_failed,
            "runtime_warning_count": runtime_warning_count,
            "max_runtime_warnings": options.max_runtime_warnings,
            "status_regressions": int_field(&regression_summary, "status_regressions"),
            "budget_regressions": int_field(&regression_summary, "budget_regressions"),
        }),
        report_path: Some(report_path.to_string()),
        regression_path: regression.map(|(path, _)| path.to_string()),
    }
}
