fn stored_message(message: &ChatMessage, index: u64) -> StoredMessage {
    StoredMessage {
        message_id: message_id(message, index),
        index,
        role: message.role.clone(),
        content: message.content.clone(),
        name: message.name.clone(),
        tool_call_id: message.tool_call_id.clone(),
        metadata: message.metadata.clone(),
    }
}

fn chat_message_from_stored(message: StoredMessage) -> ChatMessage {
    ChatMessage {
        role: message.role,
        content: message.content,
        name: message.name,
        tool_call_id: message.tool_call_id,
        metadata: message.metadata,
    }
}

fn message_id(message: &ChatMessage, index: u64) -> String {
    message
        .metadata
        .get("message_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| stable_message_id(index))
}

fn stable_message_id(index: u64) -> String {
    format!("msg_{index}")
}

fn message_parts_from_chat_message(
    session_id: &str,
    run_id: &str,
    message_id: &str,
    timestamp_ms: u64,
    message: &ChatMessage,
    index: u64,
) -> Vec<MessagePart> {
    let mut parts = Vec::new();
    let mut seq = 1_u64;
    if !message.content.is_empty() {
        parts.push(MessagePart {
            id: stable_part_id(message_id, seq, "text"),
            message_id: message_id.to_string(),
            session_id: session_id.to_string(),
            seq,
            kind: MessagePartKind::Text,
            status: MessageStatus::Completed,
            content: json!(message.content),
            attributes: BTreeMap::from([
                ("role".to_string(), json!(message.role.clone())),
                ("index".to_string(), json!(index)),
                (
                    "chars".to_string(),
                    json!(message.content.chars().count() as u64),
                ),
            ]),
            timestamp_ms,
            run_id: (!run_id.is_empty()).then(|| run_id.to_string()),
            step_index: step_index_from_metadata(&message.metadata),
        });
        seq += 1;
    }
    if message.role == Role::Assistant
        && let Some(tool_calls) = message.metadata.get("tool_calls").and_then(Value::as_array)
    {
        for call in tool_calls {
            let content = tool_call_part_content(call);
            parts.push(MessagePart {
                id: stable_part_id(message_id, seq, "tool"),
                message_id: message_id.to_string(),
                session_id: session_id.to_string(),
                seq,
                kind: MessagePartKind::Tool,
                status: MessageStatus::Pending,
                content,
                attributes: BTreeMap::from([
                    ("role".to_string(), json!("assistant")),
                    ("index".to_string(), json!(index)),
                ]),
                timestamp_ms,
                run_id: (!run_id.is_empty()).then(|| run_id.to_string()),
                step_index: step_index_from_metadata(&message.metadata),
            });
            seq += 1;
        }
    }
    parts
}

fn tool_result_part_from_message(
    session_id: &str,
    run_id: &str,
    message_id: &str,
    seq: u64,
    timestamp_ms: u64,
    message: &ChatMessage,
    index: u64,
) -> MessagePart {
    let error = message_tool_error(message);
    let output = message_tool_output(message);
    let mut attributes = BTreeMap::from([
        ("role".to_string(), json!("tool")),
        ("index".to_string(), json!(index)),
    ]);
    if let Some(name) = &message.name {
        attributes.insert("name".to_string(), json!(name));
    }
    if let Some(call_id) = &message.tool_call_id {
        attributes.insert("call_id".to_string(), json!(call_id));
    }
    MessagePart {
        id: stable_part_id(message_id, seq, "tool"),
        message_id: message_id.to_string(),
        session_id: session_id.to_string(),
        seq,
        kind: MessagePartKind::Tool,
        status: if error.is_some() {
            MessageStatus::Error
        } else {
            MessageStatus::Completed
        },
        content: json!({
            "call_id": message.tool_call_id.clone(),
            "name": message.name.clone(),
            "output": output,
            "error": error,
            "metadata": message.metadata.get("tool_result").and_then(|value| value.get("metadata")).cloned().unwrap_or_else(|| json!({})),
        }),
        attributes,
        timestamp_ms,
        run_id: (!run_id.is_empty()).then(|| run_id.to_string()),
        step_index: step_index_from_metadata(&message.metadata),
    }
}

fn stable_part_id(message_id: &str, seq: u64, kind: &str) -> String {
    format!("prt_{message_id}_{seq}_{kind}")
}

fn tool_call_part_content(call: &Value) -> Value {
    let function = call.get("function").and_then(Value::as_object);
    let name = function
        .and_then(|item| item.get("name"))
        .and_then(Value::as_str)
        .or_else(|| call.get("name").and_then(Value::as_str))
        .unwrap_or_default();
    let input = call
        .get("input")
        .cloned()
        .or_else(|| {
            function
                .and_then(|item| item.get("arguments"))
                .map(parse_json_argument)
        })
        .unwrap_or_else(|| json!({}));
    let call_id = call
        .get("call_id")
        .or_else(|| call.get("id"))
        .or_else(|| call.get("tool_call_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    json!({
        "call_id": call_id,
        "name": name,
        "input": input,
    })
}

fn parse_json_argument(value: &Value) -> Value {
    match value {
        Value::String(raw) => serde_json::from_str::<Value>(raw).unwrap_or_else(|_| {
            json!({
                "_raw": raw,
            })
        }),
        Value::Object(_) => value.clone(),
        Value::Array(_) => json!({"_value": value}),
        Value::Null => json!({}),
        other => json!({"_value": other}),
    }
}

fn tool_call_id_from_part(part: &MessagePart) -> Option<String> {
    part.content
        .get("call_id")
        .or_else(|| part.content.get("id"))
        .and_then(Value::as_str)
        .or_else(|| part.attributes.get("call_id").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn message_tool_error(message: &ChatMessage) -> Option<String> {
    message
        .metadata
        .get("tool_result")
        .and_then(|value| value.get("error"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn message_tool_output(message: &ChatMessage) -> String {
    message
        .metadata
        .get("tool_result")
        .and_then(|value| value.get("output"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| message.content.clone())
}

fn message_status_from_str(value: &str) -> MessageStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "pending" => MessageStatus::Pending,
        "running" => MessageStatus::Running,
        "error" | "failed" => MessageStatus::Error,
        "interrupted" | "cancelled" | "canceled" => MessageStatus::Interrupted,
        _ => MessageStatus::Completed,
    }
}

fn normalize_part_status(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "pending" => "pending".to_string(),
        "running" => "running".to_string(),
        "completed" => "completed".to_string(),
        "interrupted" | "cancelled" | "canceled" => "interrupted".to_string(),
        "error" | "failed" => "error".to_string(),
        _ => "ok".to_string(),
    }
}
