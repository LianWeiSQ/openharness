fn openai_chat_response_to_runtime_result(value: &Value, source: String) -> RuntimeProviderResult {
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));
    let answer = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let function = item.get("function")?;
            Some(ToolCall {
                call_id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("chat_tool_call_{index}"), str::to_string),
                name: function
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: parse_tool_arguments(
                    function
                        .get("arguments")
                        .unwrap_or(&Value::String(String::new())),
                ),
            })
        })
        .collect::<Vec<_>>();
    let usage = usage_from_provider_json(value.get("usage"));
    RuntimeProviderResult {
        answer: if answer.is_empty() && tool_calls.is_empty() {
            stable_json_dumps(value)
        } else {
            answer
        },
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or(if tool_calls.is_empty() {
                "stop"
            } else {
                "tool_call"
            })
            .to_string(),
        tool_calls,
        usage,
        source,
    }
}

fn provider_events_to_runtime_result(
    events: &[ProviderStreamEvent],
    source: String,
    fallback: Option<&Value>,
) -> RuntimeProviderResult {
    let mut answer = String::new();
    let mut tool_calls = Vec::new();
    let mut usage = Usage::default();
    let mut finish_reason = "stop".to_string();
    for event in events {
        match event {
            ProviderStreamEvent::TextDelta { text } => answer.push_str(text),
            ProviderStreamEvent::ToolCall {
                call_id,
                name,
                input,
            } => tool_calls.push(ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                input: input.clone(),
            }),
            ProviderStreamEvent::Finish {
                usage: item,
                finish_reason: reason,
            } => {
                usage = item.clone();
                finish_reason = reason.clone();
            }
        }
    }
    if answer.is_empty()
        && tool_calls.is_empty()
        && let Some(value) = fallback
    {
        answer = stable_json_dumps(value);
    }
    RuntimeProviderResult {
        answer,
        tool_calls,
        usage,
        source,
        finish_reason,
    }
}
