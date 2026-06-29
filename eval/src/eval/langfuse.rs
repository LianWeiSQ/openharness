#[must_use]
pub fn langfuse_score_payloads(
    result: &EvalResult,
    case_id: &str,
    run_id: &str,
    trace_id: &str,
) -> Vec<Value> {
    let status_comment = if result.failure_reasons.is_empty() {
        format!("OpenAgent eval status for case {case_id}.")
    } else {
        result
            .failure_reasons
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ")
    };
    vec![
        json!({
            "trace_id": trace_id,
            "score_id": format!("openagent:{run_id}:{case_id}:score"),
            "name": "openagent.eval.score",
            "value": result.score,
            "data_type": "NUMERIC",
            "comment": format!("OpenAgent eval score for case {case_id}."),
        }),
        json!({
            "trace_id": trace_id,
            "score_id": format!("openagent:{run_id}:{case_id}:status"),
            "name": "openagent.eval.status",
            "value": result.status,
            "data_type": "CATEGORICAL",
            "comment": status_comment,
        }),
        json!({
            "trace_id": trace_id,
            "score_id": format!("openagent:{run_id}:{case_id}:trace_check"),
            "name": "openagent.trace_check",
            "value": result.trace_check_ok,
            "data_type": "BOOLEAN",
            "comment": "OpenAgent trace integrity check result.",
        }),
        json!({
            "trace_id": trace_id,
            "score_id": format!("openagent:{run_id}:{case_id}:runtime_warning_count"),
            "name": "openagent.runtime_warning_count",
            "value": result.runtime_warning_count,
            "data_type": "NUMERIC",
            "comment": "OpenAgent runtime warning count for this eval case.",
        }),
    ]
}
