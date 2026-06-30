fn pending_approval_tool_call(approval: &Value) -> Result<ToolCall, String> {
    let name = approval
        .get("tool_name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "pending approval missing tool_name".to_string())?;
    let call_id = approval
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("call_approval");
    Ok(ToolCall {
        name: name.to_string(),
        input: approval
            .get("tool_input")
            .cloned()
            .unwrap_or_else(|| json!({})),
        call_id: call_id.to_string(),
    })
}

fn pending_question_tool_call(question: &Value) -> Result<ToolCall, String> {
    let name = question
        .get("tool_name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("question");
    let call_id = question
        .get("call_id")
        .or_else(|| question.get("tool_call_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("call_question");
    Ok(ToolCall {
        name: name.to_string(),
        input: question
            .get("tool_input")
            .cloned()
            .unwrap_or_else(|| json!({})),
        call_id: call_id.to_string(),
    })
}

fn approval_payload_for_tool_call(
    session: &Session,
    run_id: &str,
    step: u64,
    call: &ToolCall,
    metadata: &BTreeMap<String, Value>,
) -> Value {
    json!({
        "request_id": format!("approval_{}", call.call_id),
        "session_id": session.id,
        "turn_id": run_id,
        "run_id": run_id,
        "step": step,
        "tool_name": call.name,
        "tool_input": call.input,
        "call_id": call.call_id,
        "created_at_ms": now_ms(),
        "permission_action": metadata.get("permission_action").cloned().unwrap_or_else(|| json!("ask")),
        "permission_pattern": metadata.get("permission_pattern").cloned().unwrap_or_else(|| json!("")),
        "reason": metadata.get("error_kind").cloned().unwrap_or_else(|| json!("permission_required")),
        "metadata": metadata,
    })
}

fn question_payload_for_tool_call(
    session: &Session,
    run_id: &str,
    step: u64,
    call: &ToolCall,
) -> Value {
    json!({
        "request_id": format!("question_{}", call.call_id),
        "session_id": session.id,
        "turn_id": run_id,
        "run_id": run_id,
        "step": step,
        "tool_name": call.name,
        "tool_input": call.input,
        "tool_call_id": call.call_id,
        "call_id": call.call_id,
        "questions": call.input.get("questions").cloned().unwrap_or_else(|| json!([])),
        "created_at_ms": now_ms(),
    })
}

fn question_answers_from_json(value: &Value) -> Option<Vec<Vec<String>>> {
    let items = value.as_array()?;
    if items.iter().all(Value::is_array) {
        return Some(
            items
                .iter()
                .map(|item| {
                    item.as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(value_to_answer_string)
                        .collect::<Vec<_>>()
                })
                .collect(),
        );
    }
    Some(
        items
            .iter()
            .filter_map(value_to_answer_string)
            .map(|answer| vec![answer])
            .collect(),
    )
}

fn value_to_answer_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_bool().map(|item| item.to_string()))
        .or_else(|| value.as_i64().map(|item| item.to_string()))
        .or_else(|| value.as_u64().map(|item| item.to_string()))
        .or_else(|| value.as_f64().map(|item| item.to_string()))
}

fn tool_calls_from_turn_payload(payload: &Value) -> Result<Vec<ToolCall>, String> {
    if let Some(tool_call) = payload.get("tool_call") {
        return Ok(vec![tool_call_from_value(tool_call, 0)?]);
    }
    if let Some(items) = payload.get("tool_calls").and_then(Value::as_array) {
        return items
            .iter()
            .enumerate()
            .map(|(index, item)| tool_call_from_value(item, index))
            .collect();
    }
    Ok(Vec::new())
}

fn manual_runtime_subagent_tool_call(input: &str) -> Option<ToolCall> {
    let trimmed = input.trim_start();
    let rest = trimmed.strip_prefix('@')?;
    let (subagent_type, prompt) = rest.split_once(char::is_whitespace)?;
    let subagent_type = subagent_type.trim();
    let prompt = prompt.trim();
    if subagent_type.is_empty()
        || prompt.is_empty()
        || !subagent_type
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return None;
    }
    Some(ToolCall {
        name: TASK_TOOL_ID.to_string(),
        input: json!({
            "description": format!("@{subagent_type}"),
            "prompt": prompt,
            "subagent_type": subagent_type,
        }),
        call_id: format!("manual_task_{subagent_type}"),
    })
}

fn tool_call_from_value(value: &Value, index: usize) -> Result<ToolCall, String> {
    let name = value
        .get("name")
        .or_else(|| value.get("tool"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "tool call name is required".to_string())?;
    Ok(ToolCall {
        name: name.to_string(),
        input: value
            .get("input")
            .or_else(|| value.get("arguments"))
            .cloned()
            .unwrap_or_else(|| json!({})),
        call_id: value
            .get("call_id")
            .or_else(|| value.get("id"))
            .and_then(Value::as_str)
            .map_or_else(|| format!("call_{index}"), str::to_string),
    })
}
