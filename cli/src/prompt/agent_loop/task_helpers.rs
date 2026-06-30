fn validate_task_resume_session(
    child_session: &Session,
    parent_session: &Session,
    profile: &RunAgentProfile,
    requested_subagent_type: &str,
    task_id: &str,
) -> Result<(), String> {
    if !child_session
        .metadata
        .get("subagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(format!("task session {task_id} is not a subagent task"));
    }
    let parent_id = child_session
        .metadata
        .get("parent_session_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if parent_id != parent_session.id {
        return Err("task does not belong to parent session".to_string());
    }
    let stored_agent = child_session
        .metadata
        .get("agent")
        .and_then(Value::as_str)
        .or_else(|| {
            child_session
                .metadata
                .get("task_subagent_type")
                .and_then(Value::as_str)
        })
        .unwrap_or_default();
    if !stored_agent.is_empty()
        && stored_agent != profile.id
        && stored_agent != requested_subagent_type
    {
        return Err(format!(
            "task session {task_id} belongs to subagent {stored_agent}, not {}",
            profile.id
        ));
    }
    match child_session
        .metadata
        .get("task_status")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "queued" | "running" | "canceled" => {
            return Err(format!(
                "task session {task_id} cannot be resumed while task status is {}",
                child_session
                    .metadata
                    .get("task_status")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ));
        }
        _ => {}
    }
    if matches!(
        child_session.status,
        SessionStatus::Running | SessionStatus::Paused | SessionStatus::Compacting
    ) {
        return Err(format!(
            "task session {task_id} cannot be resumed while session status is {}",
            task_session_status_text(&child_session.status)
        ));
    }
    Ok(())
}

fn task_session_status_text(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::Paused => "paused",
        SessionStatus::Stop => "stop",
        SessionStatus::Compacting => "compacting",
    }
}

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
