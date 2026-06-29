#[must_use]
pub fn compare_with_baseline(
    baseline_report_path: &str,
    baseline_report: &Value,
    current_results: &[EvalResult],
    current_report_path: &str,
    thresholds: Option<&Value>,
) -> Value {
    let baseline_results = baseline_report
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let raw_thresholds = thresholds.or_else(|| baseline_report.get("regression_thresholds"));
    let threshold_config = normalize_regression_thresholds(raw_thresholds);

    let baseline_by_id = baseline_results
        .iter()
        .filter_map(|item| {
            let case_id = item.get("case_id")?.as_str()?;
            Some((case_id.to_string(), item.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let current_by_id = current_results
        .iter()
        .filter_map(|result| {
            serde_json::to_value(result)
                .ok()
                .map(|value| (result.case_id.clone(), value))
        })
        .collect::<BTreeMap<_, _>>();

    let mut case_ids = baseline_by_id.keys().cloned().collect::<BTreeSet<_>>();
    case_ids.extend(current_by_id.keys().cloned());
    let mut cases = Vec::new();
    for case_id in case_ids {
        let baseline = baseline_by_id.get(&case_id);
        let current = current_by_id.get(&case_id);
        match (baseline, current) {
            (None, Some(current_value)) => cases.push(json!({
                "case_id": case_id,
                "change": "new_case",
                "current": case_regression_fields(current_value),
            })),
            (Some(baseline_value), None) => cases.push(json!({
                "case_id": case_id,
                "change": "removed_case",
                "baseline": case_regression_fields(baseline_value),
            })),
            (Some(baseline_value), Some(current_value)) => {
                let score_delta =
                    float_field(current_value, "score") - float_field(baseline_value, "score");
                let cost_delta =
                    float_field(current_value, "cost") - float_field(baseline_value, "cost");
                let duration_delta_ms = int_field(current_value, "duration_ms")
                    - int_field(baseline_value, "duration_ms");
                let input_tokens_delta = int_field(current_value, "input_tokens")
                    - int_field(baseline_value, "input_tokens");
                let output_tokens_delta = int_field(current_value, "output_tokens")
                    - int_field(baseline_value, "output_tokens");
                let total_tokens_delta = (int_field(current_value, "input_tokens")
                    + int_field(current_value, "output_tokens"))
                    - (int_field(baseline_value, "input_tokens")
                        + int_field(baseline_value, "output_tokens"));
                let tool_calls_delta = int_field(current_value, "tool_calls")
                    - int_field(baseline_value, "tool_calls");
                let model_calls_delta = int_field(current_value, "model_calls")
                    - int_field(baseline_value, "model_calls");
                let baseline_status = string_field(baseline_value, "status");
                let current_status = string_field(current_value, "status");
                let budget_regressions = budget_regressions(
                    &[
                        ("cost_delta", cost_delta),
                        ("duration_delta_ms", duration_delta_ms as f64),
                        ("input_tokens_delta", input_tokens_delta as f64),
                        ("output_tokens_delta", output_tokens_delta as f64),
                        ("total_tokens_delta", total_tokens_delta as f64),
                        ("tool_calls_delta", tool_calls_delta as f64),
                        ("model_calls_delta", model_calls_delta as f64),
                    ],
                    &threshold_config,
                );
                cases.push(json!({
                    "case_id": case_id,
                    "change": "compared",
                    "baseline": case_regression_fields(baseline_value),
                    "current": case_regression_fields(current_value),
                    "status_changed": baseline_status != current_status,
                    "status_regression": baseline_status == "pass" && current_status != "pass",
                    "status_improvement": baseline_status != "pass" && current_status == "pass",
                    "score_delta": score_delta,
                    "cost_delta": cost_delta,
                    "duration_delta_ms": duration_delta_ms,
                    "input_tokens_delta": input_tokens_delta,
                    "output_tokens_delta": output_tokens_delta,
                    "total_tokens_delta": total_tokens_delta,
                    "tool_calls_delta": tool_calls_delta,
                    "model_calls_delta": model_calls_delta,
                    "budget_regressions": budget_regressions,
                }));
            }
            (None, None) => {}
        }
    }

    let threshold_value = threshold_config
        .iter()
        .map(|(key, value)| (key.clone(), json!(value)))
        .collect::<Map<_, _>>();
    let summary = json!({
        "baseline_report": baseline_report_path,
        "current_report": current_report_path,
        "regression_thresholds": threshold_value,
        "baseline_total": baseline_by_id.len() as i64,
        "current_total": current_by_id.len() as i64,
        "new_cases": cases.iter().filter(|item| item["change"] == "new_case").count() as i64,
        "removed_cases": cases.iter().filter(|item| item["change"] == "removed_case").count() as i64,
        "status_regressions": cases.iter().filter(|item| bool_field(item, "status_regression")).count() as i64,
        "status_improvements": cases.iter().filter(|item| bool_field(item, "status_improvement")).count() as i64,
        "score_regressions": cases.iter().filter(|item| float_field(item, "score_delta") < 0.0).count() as i64,
        "cost_increased_cases": cases.iter().filter(|item| float_field(item, "cost_delta") > 0.0).count() as i64,
        "input_tokens_increased_cases": cases.iter().filter(|item| int_field(item, "input_tokens_delta") > 0).count() as i64,
        "output_tokens_increased_cases": cases.iter().filter(|item| int_field(item, "output_tokens_delta") > 0).count() as i64,
        "total_tokens_increased_cases": cases.iter().filter(|item| int_field(item, "total_tokens_delta") > 0).count() as i64,
        "duration_increased_cases": cases.iter().filter(|item| int_field(item, "duration_delta_ms") > 0).count() as i64,
        "budget_regressions": cases.iter().filter(|item| {
            item.get("budget_regressions")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
        }).count() as i64,
    });

    json!({"summary": summary, "cases": cases})
}

#[must_use]
pub fn render_regression_summary(regression: &Value) -> String {
    let summary = regression.get("summary").and_then(Value::as_object);
    let summary_value = regression.get("summary").unwrap_or(&Value::Null);
    let mut lines = vec![
        "# OpenAgent Eval Regression".to_string(),
        String::new(),
        format!(
            "- Baseline cases: {}",
            int_field(summary_value, "baseline_total")
        ),
        format!(
            "- Current cases: {}",
            int_field(summary_value, "current_total")
        ),
        format!("- New cases: {}", int_field(summary_value, "new_cases")),
        format!(
            "- Removed cases: {}",
            int_field(summary_value, "removed_cases")
        ),
        format!(
            "- Status regressions: {}",
            int_field(summary_value, "status_regressions")
        ),
        format!(
            "- Status improvements: {}",
            int_field(summary_value, "status_improvements")
        ),
        format!(
            "- Score regressions: {}",
            int_field(summary_value, "score_regressions")
        ),
        format!(
            "- Cost increased cases: {}",
            int_field(summary_value, "cost_increased_cases")
        ),
        format!(
            "- Token increased cases: {}",
            int_field(summary_value, "total_tokens_increased_cases")
        ),
        format!(
            "- Duration increased cases: {}",
            int_field(summary_value, "duration_increased_cases")
        ),
        format!(
            "- Budget regressions: {}",
            int_field(summary_value, "budget_regressions")
        ),
    ];

    if let Some(thresholds) = summary
        .and_then(|item| item.get("regression_thresholds"))
        .and_then(Value::as_object)
        .filter(|item| !item.is_empty())
    {
        lines.push(String::new());
        lines.push("## Regression Thresholds".to_string());
        let mut keys = thresholds.keys().collect::<Vec<_>>();
        keys.sort();
        for key in keys {
            lines.push(format!(
                "- {key}: {}",
                format_g(float_value(&thresholds[key]))
            ));
        }
    }

    let interesting = regression
        .get("cases")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    string_field(item, "change") != "compared"
                        || bool_field(item, "status_changed")
                        || float_field(item, "score_delta") != 0.0
                        || float_field(item, "cost_delta") != 0.0
                        || int_field(item, "total_tokens_delta") != 0
                        || int_field(item, "duration_delta_ms") != 0
                        || item
                            .get("budget_regressions")
                            .and_then(Value::as_array)
                            .is_some_and(|items| !items.is_empty())
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !interesting.is_empty() {
        lines.push(String::new());
        lines.push("## Case Changes".to_string());
        for item in interesting {
            let case_id = string_field(&item, "case_id");
            let change = string_field(&item, "change");
            if change != "compared" {
                lines.push(format!("- {case_id}: {change}"));
                continue;
            }
            let baseline_status = item
                .get("baseline")
                .map(|value| string_field(value, "status"))
                .unwrap_or_default();
            let current_status = item
                .get("current")
                .map(|value| string_field(value, "status"))
                .unwrap_or_default();
            lines.push(format!(
                "- {case_id}: {baseline_status} -> {current_status}, score_delta={:.3}, cost_delta={:.6}, tokens_delta={}, duration_delta_ms={}",
                float_field(&item, "score_delta"),
                float_field(&item, "cost_delta"),
                int_field(&item, "total_tokens_delta"),
                int_field(&item, "duration_delta_ms"),
            ));
            if let Some(reasons) = item.get("budget_regressions").and_then(Value::as_array) {
                for reason in reasons.iter().filter_map(Value::as_str) {
                    lines.push(format!("  - budget: {reason}"));
                }
            }
        }
    }
    lines.join("\n") + "\n"
}
