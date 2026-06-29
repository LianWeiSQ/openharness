#[must_use]
pub fn aggregate_results(results: &[EvalResult]) -> Value {
    let total = results.len() as i64;
    let passed = results
        .iter()
        .filter(|result| result.status == "pass")
        .count() as i64;
    json!({
        "total_cases": total,
        "passed": passed,
        "failed": total - passed,
        "success_rate": if total == 0 { 0.0 } else { passed as f64 / total as f64 },
        "average_steps": average(results.iter().map(|result| result.steps as f64)),
        "average_model_calls": average(results.iter().map(|result| result.model_calls as f64)),
        "average_tool_calls": average(results.iter().map(|result| result.tool_calls as f64)),
        "total_input_tokens": results.iter().map(|result| result.input_tokens).sum::<i64>(),
        "total_output_tokens": results.iter().map(|result| result.output_tokens).sum::<i64>(),
        "total_cost": results.iter().map(|result| result.cost).sum::<f64>(),
        "average_duration_ms": average(results.iter().map(|result| result.duration_ms as f64)),
        "average_total_latency_ms": average(results.iter().map(|result| result.total_latency_ms as f64)),
        "trace_check_passed": results.iter().filter(|result| result.trace_check_ok).count() as i64,
        "trace_check_failed": results.iter().filter(|result| !result.trace_check_ok).count() as i64,
        "mcp_calls": results.iter().map(|result| result.mcp_calls).sum::<i64>(),
        "skill_calls": results.iter().map(|result| result.skill_calls).sum::<i64>(),
        "local_tool_calls": results.iter().map(|result| result.local_tool_calls).sum::<i64>(),
        "artifact_count": results.iter().map(|result| result.artifact_count).sum::<i64>(),
        "error_count": results.iter().map(|result| result.error_count).sum::<i64>(),
        "runtime_warning_count": results.iter().map(|result| result.runtime_warning_count).sum::<i64>(),
    })
}

#[must_use]
pub fn render_summary(results: &[EvalResult]) -> String {
    let aggregate = aggregate_results(results);
    let total = int_value(&aggregate["total_cases"]);
    let passed = int_value(&aggregate["passed"]);
    let mut lines = vec![
        "# OpenAgent Eval Summary".to_string(),
        String::new(),
        format!("- Total cases: {total}"),
        format!("- Passed: {passed}"),
        format!("- Failed: {}", total - passed),
        format!(
            "- Success rate: {:.1}%",
            if total == 0 {
                0.0
            } else {
                passed as f64 / total as f64 * 100.0
            }
        ),
        format!(
            "- Average steps: {:.2}",
            float_value(&aggregate["average_steps"])
        ),
        format!(
            "- Average model calls: {:.2}",
            float_value(&aggregate["average_model_calls"])
        ),
        format!(
            "- Average tool calls: {:.2}",
            float_value(&aggregate["average_tool_calls"])
        ),
        format!(
            "- Total input tokens: {}",
            int_value(&aggregate["total_input_tokens"])
        ),
        format!(
            "- Total output tokens: {}",
            int_value(&aggregate["total_output_tokens"])
        ),
        format!("- Total cost: {:.6}", float_value(&aggregate["total_cost"])),
        format!(
            "- Average duration: {:.2} ms",
            float_value(&aggregate["average_duration_ms"])
        ),
        format!(
            "- Trace checks passed: {}",
            int_value(&aggregate["trace_check_passed"])
        ),
        format!(
            "- Trace checks failed: {}",
            int_value(&aggregate["trace_check_failed"])
        ),
        format!(
            "- Runtime warnings: {}",
            int_value(&aggregate["runtime_warning_count"])
        ),
        format!(
            "- Tool sources: local={} skill={} mcp={}",
            int_value(&aggregate["local_tool_calls"]),
            int_value(&aggregate["skill_calls"]),
            int_value(&aggregate["mcp_calls"])
        ),
    ];

    if let Some(slowest) = results.iter().max_by_key(|result| result.duration_ms) {
        lines.push(format!(
            "- Slowest case: {} ({} ms)",
            slowest.case_id, slowest.duration_ms
        ));
    }
    if let Some(priciest) = results
        .iter()
        .max_by(|left, right| compare_f64(left.cost, right.cost))
    {
        lines.push(format!(
            "- Most expensive case: {} ({:.6})",
            priciest.case_id, priciest.cost
        ));
    }

    let failures = results
        .iter()
        .flat_map(|result| result.failure_reasons.iter().cloned())
        .collect::<BTreeSet<_>>();
    if !failures.is_empty() {
        lines.push(String::new());
        lines.push("## Failure Reasons".to_string());
        lines.extend(failures.into_iter().map(|reason| format!("- {reason}")));
    }
    lines.join("\n") + "\n"
}
