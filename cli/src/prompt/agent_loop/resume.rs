pub(super) fn pending_resume_from_session(session: &Session) -> Option<PendingResume> {
    if let Some(response) = session.metadata.get("pending_question_response")
        && let Some(pending) = session.metadata.get("pending_question")
    {
        return pending_resume_from_values("question", pending, response);
    }
    if let Some(response) = session.metadata.get("pending_approval_response")
        && let Some(pending) = session.metadata.get("pending_approval")
    {
        return pending_resume_from_values("approval", pending, response);
    }
    None
}

fn pending_resume_from_values(
    kind: &str,
    pending: &Value,
    response: &Value,
) -> Option<PendingResume> {
    let call_id = pending.get("call_id").and_then(Value::as_str)?.to_string();
    let tool_name = pending
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or(if kind == "question" { "question" } else { "" })
        .to_string();
    if tool_name.is_empty() {
        return None;
    }
    Some(PendingResume {
        kind: kind.to_string(),
        request_id: pending
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or(&call_id)
            .to_string(),
        call: ToolCall {
            name: tool_name,
            input: pending
                .get("tool_input")
                .or_else(|| pending.get("toolInput"))
                .cloned()
                .unwrap_or_else(|| json!({})),
            call_id,
        },
        response: response.clone(),
        step: pending.get("step").and_then(Value::as_u64).unwrap_or(0),
    })
}
