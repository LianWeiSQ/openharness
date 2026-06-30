fn openai_stream_text_delta(wire_api: &str, chunk: &Value) -> Option<ProviderStreamEvent> {
    let text = if wire_api == "chat" {
        chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("content"))
            .or_else(|| {
                chunk
                    .get("choices")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(|choice| choice.get("text"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else if matches!(
        chunk.get("type").and_then(Value::as_str),
        Some("response.output_text.delta" | "response.refusal.delta")
    ) {
        chunk
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else {
        ""
    };
    (!text.is_empty()).then(|| ProviderStreamEvent::TextDelta {
        text: text.to_string(),
    })
}

fn anthropic_stream_text_delta(chunk: &Value) -> Option<ProviderStreamEvent> {
    let text = match chunk
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "content_block_start" => chunk
            .get("content_block")
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|block| block.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "content_block_delta" => chunk
            .get("delta")
            .filter(|delta| delta.get("type").and_then(Value::as_str) == Some("text_delta"))
            .and_then(|delta| delta.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        _ => "",
    };
    (!text.is_empty()).then(|| ProviderStreamEvent::TextDelta {
        text: text.to_string(),
    })
}

fn normalize_openai_responses_stream_events(chunks: &[Value]) -> Vec<ProviderStreamEvent> {
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
