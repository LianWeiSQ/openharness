fn provider_events_to_run_result(
    events: &[ProviderStreamEvent],
    source: String,
    fallback_json: Option<&Value>,
) -> ProviderRunResult {
    let mut answer = String::new();
    let mut usage = Usage::default();
    let mut tool_calls = Vec::new();
    let mut finish_reason = "stop".to_string();
    for event in events {
        match event {
            ProviderStreamEvent::TextDelta { text } => answer.push_str(text),
            ProviderStreamEvent::Finish {
                usage: item,
                finish_reason: reason,
            } => {
                usage = item.clone();
                finish_reason = reason.clone();
            }
            ProviderStreamEvent::ToolCall {
                call_id,
                name,
                input,
            } => {
                tool_calls.push(ToolCall {
                    name: name.clone(),
                    input: input.clone(),
                    call_id: call_id.clone(),
                });
            }
        }
    }
    if answer.is_empty()
        && tool_calls.is_empty()
        && let Some(value) = fallback_json
    {
        answer = stable_json_dumps(value);
    }
    ProviderRunResult {
        answer,
        tool_calls,
        usage,
        source,
        finish_reason,
    }
}

fn extract_chat_answer(value: &Value) -> String {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn extract_chat_tool_calls(value: &Value) -> Vec<ToolCall> {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("message"))
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, call)| {
            let function = call.get("function");
            let name = function
                .and_then(|item| item.get("name"))
                .or_else(|| call.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            let call_id = call
                .get("id")
                .or_else(|| call.get("call_id"))
                .and_then(Value::as_str)
                .map_or_else(|| format!("chat_call_{index}"), str::to_string);
            let arguments = function
                .and_then(|item| item.get("arguments"))
                .or_else(|| call.get("arguments"))
                .or_else(|| call.get("input"))
                .unwrap_or(&Value::Null);
            Some(ToolCall {
                name: name.to_string(),
                input: parse_tool_arguments(arguments),
                call_id,
            })
        })
        .collect()
}

fn extract_chat_finish_reason(value: &Value) -> Option<String> {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("finish_reason"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn extract_anthropic_tool_calls(value: &Value) -> Vec<ToolCall> {
    value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, item)| {
            if item.get("type").and_then(Value::as_str) != Some("tool_use") {
                return None;
            }
            let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            Some(ToolCall {
                name: name.to_string(),
                input: item.get("input").cloned().unwrap_or_else(|| json!({})),
                call_id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("toolu_{index}"), str::to_string),
            })
        })
        .collect()
}

fn usage_from_json(value: Option<&Value>) -> Usage {
    let Some(value) = value else {
        return Usage::default();
    };
    Usage {
        input_tokens: value
            .get("input_tokens")
            .or_else(|| value.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        output_tokens: value
            .get("output_tokens")
            .or_else(|| value.get("completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        cost: 0.0,
    }
}

pub(super) fn add_usage(total: &mut Usage, item: &Usage) {
    total.input_tokens += item.input_tokens;
    total.output_tokens += item.output_tokens;
    total.cost += item.cost;
}
