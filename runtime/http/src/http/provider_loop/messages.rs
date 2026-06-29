fn runtime_chat_message(role: Role, content: String) -> ChatMessage {
    ChatMessage {
        role,
        content,
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::new(),
    }
}

fn runtime_message_id(index: u64) -> String {
    format!("msg_{index}")
}

fn latest_assistant_message_id_for_tool(session: &Session, tool_call: &ToolCall) -> Option<String> {
    session.messages.iter().rev().find_map(|message| {
        if message.role != Role::Assistant {
            return None;
        }
        let message_id = message
            .metadata
            .get("message_id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let has_call = message
            .metadata
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| {
                calls.iter().any(|call| {
                    call.get("call_id")
                        .or_else(|| call.get("id"))
                        .and_then(Value::as_str)
                        == Some(tool_call.call_id.as_str())
                })
            });
        if has_call { message_id } else { None }
    })
}

fn assistant_message_for_provider_step(content: String, tool_calls: &[ToolCall]) -> ChatMessage {
    let mut message = runtime_chat_message(Role::Assistant, content);
    if !tool_calls.is_empty() {
        message.metadata.insert(
            "tool_calls".to_string(),
            Value::Array(tool_calls.iter().map(openai_tool_call_value).collect()),
        );
    }
    message
}

fn openai_tool_call_value(call: &ToolCall) -> Value {
    json!({
        "id": call.call_id.clone(),
        "call_id": call.call_id.clone(),
        "type": "function",
        "function": {
            "name": call.name.clone(),
            "arguments": stable_json_dumps(&call.input),
        },
        "name": call.name.clone(),
        "input": call.input.clone(),
    })
}
