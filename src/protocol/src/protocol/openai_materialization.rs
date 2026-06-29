#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenAiFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenAiTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OpenAiFunction,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MaterializedPayload {
    pub messages: Vec<Value>,
    pub tools: Vec<OpenAiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_options: BTreeMap<String, Value>,
}

#[must_use]
pub fn materialize_openai_compatible_payload(
    system: Option<&str>,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    model: Option<&Model>,
    options: Option<&BTreeMap<String, Value>>,
) -> MaterializedPayload {
    MaterializedPayload {
        messages: materialize_openai_compatible_messages(system, messages),
        tools: materialize_openai_compatible_tools(tools),
        model: model.map(|item| item.id.clone()),
        provider_options: provider_options(options),
    }
}

#[must_use]
pub fn materialize_openai_compatible_payload_from_parts(
    system: Option<&str>,
    messages: &[MessageWithParts],
    tools: &[ToolSchema],
    model: Option<&Model>,
    options: Option<&BTreeMap<String, Value>>,
) -> MaterializedPayload {
    MaterializedPayload {
        messages: materialize_model_messages(system, messages),
        tools: materialize_openai_compatible_tools(tools),
        model: model.map(|item| item.id.clone()),
        provider_options: provider_options(options),
    }
}

#[must_use]
pub fn materialize_model_messages(
    system: Option<&str>,
    messages: &[MessageWithParts],
) -> Vec<Value> {
    let legacy_messages = message_parts_to_chat_messages(messages);
    materialize_openai_compatible_messages(system, &legacy_messages)
}

#[must_use]
pub fn message_parts_to_chat_messages(messages: &[MessageWithParts]) -> Vec<ChatMessage> {
    let mut normalized = Vec::new();
    for message in messages {
        match message.info.role {
            Role::Assistant => {
                let text = text_content_from_parts(&message.parts);
                let tool_states = tool_states_from_parts(&message.parts);
                let mut metadata = message.info.metadata.clone();
                metadata
                    .entry("message_id".to_string())
                    .or_insert_with(|| json_value_string(message.info.id.clone()));
                let tool_calls = tool_states
                    .iter()
                    .filter_map(ToolPartProjection::tool_call_value)
                    .collect::<Vec<_>>();
                if !tool_calls.is_empty() {
                    metadata.insert("tool_calls".to_string(), Value::Array(tool_calls));
                }
                if !text.is_empty() || !tool_states.is_empty() {
                    normalized.push(ChatMessage {
                        role: Role::Assistant,
                        content: text,
                        name: None,
                        tool_call_id: None,
                        metadata,
                    });
                }
                for tool_state in tool_states {
                    if let Some(tool_message) = tool_state.result_message() {
                        normalized.push(tool_message);
                    }
                }
            }
            Role::Tool => {
                if let Some(tool_message) = tool_message_from_parts(message) {
                    normalized.push(tool_message);
                }
            }
            Role::System | Role::User => {
                let content = text_content_from_parts(&message.parts);
                if content.is_empty() {
                    continue;
                }
                let mut metadata = message.info.metadata.clone();
                metadata
                    .entry("message_id".to_string())
                    .or_insert_with(|| json_value_string(message.info.id.clone()));
                normalized.push(ChatMessage {
                    role: message.info.role.clone(),
                    content,
                    name: None,
                    tool_call_id: None,
                    metadata,
                });
            }
        }
    }
    normalized
}

#[must_use]
pub fn materialize_openai_compatible_messages(
    system: Option<&str>,
    messages: &[ChatMessage],
) -> Vec<Value> {
    let mut normalized = Vec::new();
    if let Some(system) = system {
        normalized.push(json_object([
            ("role", Value::String("system".to_string())),
            ("content", Value::String(system.to_string())),
        ]));
    }

    for message in messages {
        let role = serde_json::to_value(&message.role).expect("role serializes");
        let mut item = Map::from_iter([
            ("role".to_string(), role),
            (
                "content".to_string(),
                Value::String(message.content.clone()),
            ),
        ]);
        if message.role != Role::Tool
            && let Some(name) = &message.name
        {
            item.insert("name".to_string(), Value::String(name.clone()));
        }
        if let Some(tool_call_id) = &message.tool_call_id {
            item.insert(
                "tool_call_id".to_string(),
                Value::String(tool_call_id.clone()),
            );
        }
        if message.role == Role::Assistant
            && let Some(tool_calls) = message.metadata.get("tool_calls")
            && matches!(tool_calls, Value::Array(items) if !items.is_empty())
        {
            item.insert("tool_calls".to_string(), tool_calls.clone());
            if message.content.is_empty() {
                item.insert("content".to_string(), Value::Null);
            }
        }
        normalized.push(Value::Object(item));
    }
    normalized
}
