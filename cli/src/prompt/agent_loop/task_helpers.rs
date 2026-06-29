fn task_input_string(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("task tool requires non-empty {key}"))
}

fn task_tool_error(
    tool_call: &ToolCall,
    error: &str,
    mut metadata: BTreeMap<String, Value>,
) -> ToolResult {
    metadata
        .entry("tool".to_string())
        .or_insert_with(|| json!(TASK_TOOL_ID));
    ToolResult {
        call_id: tool_call.call_id.clone(),
        output: String::new(),
        error: Some(error.to_string()),
        metadata,
    }
}

fn render_task_output(task_id: &str, state: &str, text: &str) -> String {
    format!(
        "<task id=\"{}\" state=\"{}\">\n<task_result>\n{}\n</task_result>\n</task>",
        escape_task_text(task_id),
        escape_task_text(state),
        escape_task_text(text),
    )
}

fn escape_task_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

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
