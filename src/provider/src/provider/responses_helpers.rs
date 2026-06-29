fn materialize_responses_input(_system: Option<&str>, messages: &[ChatMessage]) -> Value {
    let mut normalized = Vec::new();
    for message in messages {
        let content = message.content.clone();
        match message.role {
            Role::Tool => {
                if let Some(call_id) = message
                    .tool_call_id
                    .as_ref()
                    .filter(|value| !value.is_empty())
                {
                    normalized.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": content,
                    }));
                }
            }
            Role::Assistant => {
                let tool_calls = message
                    .metadata
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .filter(|items| !items.is_empty());
                if let Some(tool_calls) = tool_calls {
                    for call in tool_calls {
                        if let Some(item) = responses_function_call_item(call) {
                            normalized.push(item);
                        }
                    }
                    if !content.is_empty() {
                        normalized.push(json!({"role": "assistant", "content": content}));
                    }
                    continue;
                }
                if !content.is_empty() {
                    normalized.push(json!({"role": "assistant", "content": content}));
                }
            }
            Role::User => {
                if !content.is_empty() {
                    normalized.push(json!({"role": "user", "content": content}));
                }
            }
            Role::System => {}
        }
    }
    Value::Array(normalized)
}

fn responses_function_call_item(call: &Value) -> Option<Value> {
    let call = call.as_object()?;
    let function = call.get("function").and_then(Value::as_object);
    let name = function
        .and_then(|item| item.get("name"))
        .and_then(Value::as_str)
        .or_else(|| call.get("name").and_then(Value::as_str))
        .unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    let arguments = function
        .and_then(|item| item.get("arguments"))
        .cloned()
        .unwrap_or(Value::Null);
    let arguments = arguments.as_str().map_or_else(
        || {
            if arguments.is_null() {
                "{}".to_string()
            } else {
                serde_json::to_string(&arguments).unwrap_or_else(|_error| "{}".to_string())
            }
        },
        str::to_string,
    );
    let call_id = call
        .get("id")
        .or_else(|| call.get("call_id"))
        .and_then(Value::as_str)
        .or_else(|| call.get("tool_call_id").and_then(Value::as_str))
        .unwrap_or_default();
    if call_id.is_empty() {
        return None;
    }
    Some(json!({
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": arguments,
    }))
}

fn materialize_responses_tools(tools: &[ToolSchema]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.schema.clone().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                })
            })
            .collect(),
    )
}

fn extract_responses_text(value: &Value) -> String {
    if !value.is_object() {
        return String::new();
    }
    if let Some(output_text) = value.get("output_text").and_then(Value::as_str)
        && !output_text.is_empty()
    {
        return output_text.to_string();
    }
    let mut texts = Vec::new();
    if let Some(output) = value.get("output").and_then(Value::as_array) {
        for item in output {
            if !item.is_object() {
                continue;
            }
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for part in content {
                    if !part.is_object() {
                        continue;
                    }
                    let extracted = extract_text_content(part);
                    if !extracted.is_empty() {
                        texts.push(extracted);
                    }
                }
            } else {
                let extracted = extract_text_content(item);
                if !extracted.is_empty() {
                    texts.push(extracted);
                }
            }
        }
    }
    if !texts.is_empty() {
        return texts.join("");
    }
    if let Some(first) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    {
        return extract_choice_text(first);
    }
    String::new()
}

fn extract_responses_tool_calls(value: &Value) -> Vec<ParsedToolCall> {
    let Some(output) = value.get("output").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut parsed = Vec::new();
    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            continue;
        }
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let call_id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map_or_else(
                || format!("responses_call_{}", parsed.len()),
                str::to_string,
            );
        parsed.push(ParsedToolCall {
            call_id,
            name,
            input: parse_tool_arguments(item.get("arguments").unwrap_or(&Value::Null)),
        });
    }
    parsed
}

fn parse_json_object(value: &Value) -> Value {
    match value {
        Value::Object(_) => value.clone(),
        Value::Array(_) => json!({ "_value": value }),
        Value::String(raw) => {
            let stripped = raw.trim();
            if stripped.is_empty() {
                return json!({});
            }
            match serde_json::from_str::<Value>(stripped) {
                Ok(Value::Object(item)) => Value::Object(item),
                Ok(value) => json!({ "_value": value }),
                Err(_) => json!({ "_raw": raw }),
            }
        }
        _ => json!({}),
    }
}
