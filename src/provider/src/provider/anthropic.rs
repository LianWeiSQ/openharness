#[must_use]
pub fn build_anthropic_payload(
    config: &AnthropicLanguageModelConfig,
    system: Option<&str>,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    temperature: Option<f64>,
    max_output_tokens: Option<u64>,
    options: Option<&BTreeMap<String, Value>>,
) -> Value {
    let mut payload = Map::from_iter([
        ("model".to_string(), json!(config.model_id)),
        (
            "messages".to_string(),
            materialize_anthropic_messages(messages),
        ),
        (
            "max_tokens".to_string(),
            json!(max_output_tokens.unwrap_or(config.max_output)),
        ),
        ("stream".to_string(), json!(true)),
    ]);
    if let Some(system) = system.filter(|value| !value.is_empty()) {
        payload.insert("system".to_string(), json!(system));
    }
    if !tools.is_empty() {
        payload.insert("tools".to_string(), materialize_anthropic_tools(tools));
        payload.insert("tool_choice".to_string(), json!({"type": "auto"}));
    }
    if let Some(temperature) = temperature {
        payload.insert("temperature".to_string(), json!(temperature));
    }
    for (key, value) in provider_options(options) {
        payload.insert(key, value);
    }
    Value::Object(payload)
}

#[must_use]
pub fn normalize_anthropic_events(source_events: &[Value]) -> Vec<ProviderStreamEvent> {
    let mut events = Vec::new();
    let mut tool_uses: BTreeMap<u64, ToolUseState> = BTreeMap::new();
    let mut finish_reason_raw = Value::Null;
    let mut input_tokens = 0;
    let mut output_tokens = 0;

    for event in source_events {
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "message_start" => {
                if let Some(value) = event
                    .get("message")
                    .and_then(|message| message.get("usage"))
                    .and_then(|usage| usage.get("input_tokens"))
                    .and_then(value_to_u64)
                {
                    input_tokens = value;
                }
            }
            "content_block_start" => {
                let index = event.get("index").and_then(value_to_u64).unwrap_or(0);
                let block = event.get("content_block").unwrap_or(&Value::Null);
                match block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "text" => {
                        let text = block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if !text.is_empty() {
                            events.push(ProviderStreamEvent::TextDelta { text });
                        }
                    }
                    "tool_use" => {
                        tool_uses.insert(
                            index,
                            ToolUseState {
                                call_id: block
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .map_or_else(|| format!("toolu_{index}"), str::to_string),
                                name: block
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                input_value: block.get("input").cloned().unwrap_or(Value::Null),
                                partial_json: String::new(),
                                emitted: false,
                            },
                        );
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let index = event.get("index").and_then(value_to_u64).unwrap_or(0);
                let delta = event.get("delta").unwrap_or(&Value::Null);
                match delta
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "text_delta" => {
                        let text = delta
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if !text.is_empty() {
                            events.push(ProviderStreamEvent::TextDelta { text });
                        }
                    }
                    "input_json_delta" => {
                        let state = tool_uses.entry(index).or_insert_with(|| ToolUseState {
                            call_id: format!("toolu_{index}"),
                            name: String::new(),
                            input_value: Value::Null,
                            partial_json: String::new(),
                            emitted: false,
                        });
                        state.partial_json.push_str(
                            delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        );
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = event.get("index").and_then(value_to_u64).unwrap_or(0);
                emit_anthropic_tool(&mut events, &mut tool_uses, index);
            }
            "message_delta" => {
                if let Some(reason) = event
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                {
                    finish_reason_raw = reason.clone();
                }
                if let Some(value) = event
                    .get("usage")
                    .and_then(|usage| usage.get("output_tokens"))
                    .and_then(value_to_u64)
                {
                    output_tokens = value;
                }
            }
            "message_stop" => break,
            _ => {}
        }
    }
    let indexes = tool_uses.keys().copied().collect::<Vec<_>>();
    for index in indexes {
        emit_anthropic_tool(&mut events, &mut tool_uses, index);
    }
    events.push(ProviderStreamEvent::Finish {
        finish_reason: map_anthropic_finish_reason(&finish_reason_raw, !tool_uses.is_empty())
            .to_string(),
        usage: Usage {
            input_tokens,
            output_tokens,
            cost: 0.0,
        },
    });
    events
}
