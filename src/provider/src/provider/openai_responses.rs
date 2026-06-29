#[must_use]
pub fn build_openai_responses_payload(
    config: &OpenAiLanguageModelConfig,
    system: Option<&str>,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    max_output_tokens: Option<u64>,
    options: Option<&BTreeMap<String, Value>>,
) -> Value {
    let mut payload = Map::from_iter([
        ("model".to_string(), json!(config.model_id)),
        (
            "input".to_string(),
            materialize_responses_input(system, messages),
        ),
        ("stream".to_string(), json!(false)),
    ]);
    if let Some(system) = system.filter(|value| !value.is_empty()) {
        payload.insert("instructions".to_string(), json!(system));
    }
    if !tools.is_empty() {
        payload.insert("tools".to_string(), materialize_responses_tools(tools));
        payload.insert("tool_choice".to_string(), json!("auto"));
    }
    if config.disable_response_storage {
        payload.insert("store".to_string(), json!(false));
    }
    if let Some(reasoning_effort) = config
        .reasoning_effort
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        payload.insert(
            "reasoning".to_string(),
            json!({ "effort": reasoning_effort }),
        );
    }
    if let Some(max_output_tokens) = max_output_tokens {
        payload.insert("max_output_tokens".to_string(), json!(max_output_tokens));
    }
    for (key, value) in provider_options(options) {
        if key != "stream" {
            payload.insert(key, value);
        }
    }
    Value::Object(payload)
}

#[must_use]
pub fn normalize_openai_responses_response(value: &Value) -> Vec<ProviderStreamEvent> {
    let mut events = Vec::new();
    let content = extract_responses_text(value);
    let tool_calls = extract_responses_tool_calls(value);
    if !content.is_empty() {
        events.push(ProviderStreamEvent::TextDelta { text: content });
    }
    for tool_call in &tool_calls {
        events.push(ProviderStreamEvent::ToolCall {
            call_id: tool_call.call_id.clone(),
            name: tool_call.name.clone(),
            input: tool_call.input.clone(),
        });
    }
    events.push(ProviderStreamEvent::Finish {
        finish_reason: if tool_calls.is_empty() {
            "stop".to_string()
        } else {
            "tool_call".to_string()
        },
        usage: usage_from_responses(value.get("usage").and_then(Value::as_object)),
    });
    events
}

#[must_use]
pub fn normalize_openai_responses_stream_events(chunks: &[Value]) -> Vec<ProviderStreamEvent> {
    let mut events = Vec::new();
    let mut finish_reason = "stop".to_string();
    let mut usage = Usage::default();
    for chunk in chunks {
        let event_type = chunk
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "response.output_text.delta" | "response.refusal.delta" => {
                let text = chunk
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !text.is_empty() {
                    events.push(ProviderStreamEvent::TextDelta {
                        text: text.to_string(),
                    });
                }
            }
            "response.output_item.done" => {
                if let Some(tool_call) =
                    response_stream_tool_call(chunk.get("item").unwrap_or(&Value::Null))
                {
                    finish_reason = "tool_call".to_string();
                    events.push(tool_call);
                }
            }
            "response.completed" => {
                if let Some(response) = chunk.get("response") {
                    let nested = normalize_openai_responses_response(response);
                    if !nested.is_empty() {
                        usage = nested
                            .iter()
                            .find_map(|event| match event {
                                ProviderStreamEvent::Finish { usage, .. } => Some(usage.clone()),
                                _ => None,
                            })
                            .unwrap_or(usage);
                    }
                }
            }
            "response.failed" | "response.incomplete" => {
                finish_reason = "error".to_string();
            }
            _ => {}
        }
    }
    events.push(ProviderStreamEvent::Finish {
        finish_reason,
        usage,
    });
    events
}

fn response_stream_tool_call(item: &Value) -> Option<ProviderStreamEvent> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    if !matches!(item_type, "function_call" | "custom_tool_call") {
        return None;
    }
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("responses_tool_call")
        .to_string();
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let input = item
        .get("arguments")
        .or_else(|| item.get("input"))
        .map(parse_tool_arguments)
        .unwrap_or_else(|| json!({}));
    Some(ProviderStreamEvent::ToolCall {
        call_id,
        name,
        input,
    })
}
