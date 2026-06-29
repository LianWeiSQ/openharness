fn materialize_anthropic_messages(messages: &[ChatMessage]) -> Value {
    let mut normalized = Vec::new();
    for message in messages {
        let content = message.content.clone();
        match message.role {
            Role::System => {}
            Role::Tool => {
                let tool_call_id = message.tool_call_id.clone().unwrap_or_default();
                if tool_call_id.is_empty() {
                    continue;
                }
                normalized.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": content,
                    }],
                }));
            }
            Role::Assistant => {
                let mut blocks = Vec::new();
                if !content.is_empty() {
                    blocks.push(json!({"type": "text", "text": content}));
                }
                if let Some(tool_calls) =
                    message.metadata.get("tool_calls").and_then(Value::as_array)
                {
                    for call in tool_calls {
                        if let Some(block) = tool_call_content_block(call) {
                            blocks.push(block);
                        }
                    }
                }
                if !blocks.is_empty() {
                    normalized.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            Role::User => {
                if !content.is_empty() {
                    normalized.push(json!({"role": "user", "content": content}));
                }
            }
        }
    }
    Value::Array(normalized)
}

fn tool_call_content_block(call: &Value) -> Option<Value> {
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
    let call_id = call
        .get("id")
        .or_else(|| call.get("call_id"))
        .or_else(|| call.get("tool_call_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if call_id.is_empty() {
        return None;
    }
    let arguments = function
        .and_then(|item| item.get("arguments"))
        .or_else(|| call.get("input"))
        .or_else(|| call.get("arguments"))
        .cloned()
        .unwrap_or(Value::Null);
    Some(json!({
        "type": "tool_use",
        "id": call_id,
        "name": name,
        "input": parse_json_object(&arguments),
    }))
}

fn materialize_anthropic_tools(tools: &[ToolSchema]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.schema.clone().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                })
            })
            .collect(),
    )
}

fn emit_anthropic_tool(
    events: &mut Vec<ProviderStreamEvent>,
    tool_uses: &mut BTreeMap<u64, ToolUseState>,
    index: u64,
) {
    let Some(state) = tool_uses.get_mut(&index) else {
        return;
    };
    if state.emitted {
        return;
    }
    state.emitted = true;
    let input = if state.partial_json.is_empty() {
        parse_json_object(&state.input_value)
    } else {
        parse_json_object(&Value::String(state.partial_json.clone()))
    };
    events.push(ProviderStreamEvent::ToolCall {
        call_id: state.call_id.clone(),
        name: state.name.clone(),
        input,
    });
}
