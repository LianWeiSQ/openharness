#[derive(Clone, Debug, Default)]
struct ToolPartProjection {
    call_id: String,
    name: String,
    input: Value,
    output: Option<String>,
    error: Option<String>,
    metadata: BTreeMap<String, Value>,
    status: MessageStatus,
}

impl ToolPartProjection {
    fn merge_part(&mut self, part: &MessagePart) {
        self.status = part.status.clone();
        if let Some(value) =
            string_from_part(part, "call_id").or_else(|| string_from_part(part, "id"))
        {
            self.call_id = value;
        }
        if let Some(value) = string_from_part(part, "name") {
            self.name = value;
        }
        if self.input.is_null()
            && let Some(value) = value_from_part(part, "input")
        {
            self.input = value;
        }
        if let Some(value) = string_from_part(part, "output") {
            self.output = Some(value);
        }
        if let Some(value) = string_from_part(part, "error") {
            self.error = Some(value);
        }
        if let Some(Value::Object(items)) = value_from_part(part, "metadata") {
            self.metadata.extend(items);
        }
        self.metadata.extend(part.attributes.clone());
    }

    fn tool_call_value(&self) -> Option<Value> {
        if self.call_id.is_empty() || self.name.is_empty() {
            return None;
        }
        Some(json_object([
            ("id", Value::String(self.call_id.clone())),
            ("call_id", Value::String(self.call_id.clone())),
            ("type", Value::String("function".to_string())),
            (
                "function",
                json_object([
                    ("name", Value::String(self.name.clone())),
                    ("arguments", Value::String(stable_json_dumps(&self.input))),
                ]),
            ),
            ("name", Value::String(self.name.clone())),
            ("input", self.input.clone()),
        ]))
    }

    fn result_message(&self) -> Option<ChatMessage> {
        if self.call_id.is_empty() {
            return None;
        }
        let pending_error = match self.status {
            MessageStatus::Pending | MessageStatus::Running | MessageStatus::Interrupted => {
                Some("Tool call interrupted before completion".to_string())
            }
            MessageStatus::Error | MessageStatus::Completed => None,
        };
        let error = self.error.clone().or(pending_error);
        let content = error.as_ref().map_or_else(
            || self.output.clone().unwrap_or_default(),
            |error| format!("Tool failed: {error}"),
        );
        let mut metadata = self.metadata.clone();
        metadata.insert(
            "tool_result".to_string(),
            json_object([
                ("call_id", Value::String(self.call_id.clone())),
                (
                    "output",
                    Value::String(self.output.clone().unwrap_or_default()),
                ),
                ("error", error.clone().map_or(Value::Null, Value::String)),
                (
                    "metadata",
                    Value::Object(metadata.clone().into_iter().collect()),
                ),
            ]),
        );
        Some(ChatMessage {
            role: Role::Tool,
            content,
            name: (!self.name.is_empty()).then(|| self.name.clone()),
            tool_call_id: Some(self.call_id.clone()),
            metadata,
        })
    }
}

fn tool_states_from_parts(parts: &[MessagePart]) -> Vec<ToolPartProjection> {
    let mut by_call_id: BTreeMap<String, ToolPartProjection> = BTreeMap::new();
    let mut anonymous = Vec::new();
    for part in parts
        .iter()
        .filter(|part| part.kind == MessagePartKind::Tool)
    {
        let call_id = string_from_part(part, "call_id")
            .or_else(|| string_from_part(part, "id"))
            .unwrap_or_default();
        if call_id.is_empty() {
            let mut state = ToolPartProjection {
                input: Value::Null,
                ..ToolPartProjection::default()
            };
            state.merge_part(part);
            anonymous.push(state);
            continue;
        }
        by_call_id
            .entry(call_id.clone())
            .or_insert_with(|| ToolPartProjection {
                call_id,
                input: Value::Null,
                ..ToolPartProjection::default()
            })
            .merge_part(part);
    }
    let mut states = by_call_id.into_values().collect::<Vec<_>>();
    states.extend(anonymous);
    states
}

fn tool_message_from_parts(message: &MessageWithParts) -> Option<ChatMessage> {
    let tool_part = message
        .parts
        .iter()
        .find(|part| part.kind == MessagePartKind::Tool)
        .or_else(|| message.parts.first())?;
    let mut state = ToolPartProjection {
        input: Value::Null,
        ..ToolPartProjection::default()
    };
    state.merge_part(tool_part);
    if state.call_id.is_empty() {
        state.call_id = message
            .info
            .metadata
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    if state.name.is_empty() {
        state.name = message
            .info
            .metadata
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    state.output = state
        .output
        .or_else(|| Some(text_content_from_parts(&message.parts)));
    state.result_message()
}

fn text_content_from_parts(parts: &[MessagePart]) -> String {
    parts
        .iter()
        .filter(|part| {
            matches!(
                part.kind,
                MessagePartKind::Text | MessagePartKind::Reasoning
            )
        })
        .filter_map(part_text)
        .collect::<Vec<_>>()
        .join("")
}

fn part_text(part: &MessagePart) -> Option<String> {
    part.content
        .as_str()
        .map(ToString::to_string)
        .or_else(|| string_from_part(part, "text"))
        .or_else(|| string_from_part(part, "content"))
}

fn string_from_part(part: &MessagePart, key: &str) -> Option<String> {
    value_from_part(part, key).and_then(|value| match value {
        Value::String(value) => Some(value),
        Value::Null => None,
        other => Some(stable_json_dumps(&other)),
    })
}

fn value_from_part(part: &MessagePart, key: &str) -> Option<Value> {
    part.content
        .get(key)
        .cloned()
        .or_else(|| part.attributes.get(key).cloned())
}

fn json_value_string(value: String) -> Value {
    Value::String(value)
}

#[must_use]
pub fn materialize_openai_compatible_tools(tools: &[ToolSchema]) -> Vec<OpenAiTool> {
    tools
        .iter()
        .map(|tool| OpenAiTool {
            kind: "function".to_string(),
            function: OpenAiFunction {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.schema.clone().unwrap_or_else(|| {
                    json_object([("type", Value::String("object".to_string()))])
                }),
            },
        })
        .collect()
}
