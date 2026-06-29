#[must_use]
pub fn build_openai_chat_payload(
    config: &OpenAiLanguageModelConfig,
    system: Option<&str>,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    temperature: Option<f64>,
    max_output_tokens: Option<u64>,
    options: Option<&BTreeMap<String, Value>>,
) -> Value {
    let model = openai_compatible_model("openai", &config.model_id);
    let payload =
        materialize_openai_compatible_payload(system, messages, tools, Some(&model), options);
    let MaterializedPayload {
        messages,
        tools,
        model,
        provider_options,
    } = payload;
    let mut item = Map::from_iter([
        ("messages".to_string(), json!(messages)),
        ("tools".to_string(), json!(tools)),
    ]);
    if let Some(model) = model {
        item.insert("model".to_string(), json!(model));
    }
    item.insert("stream".to_string(), json!(true));
    if let Some(temperature) = temperature {
        item.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_output_tokens) = max_output_tokens {
        item.insert("max_tokens".to_string(), json!(max_output_tokens));
    }
    if !tools.is_empty() {
        item.insert("tool_choice".to_string(), json!("auto"));
    }
    for (key, value) in provider_options {
        item.insert(key, value);
    }
    Value::Object(item)
}

#[must_use]
pub fn normalize_openai_chat_sse_chunks(chunks: &[Value]) -> Vec<ProviderStreamEvent> {
    let mut events = Vec::new();
    let mut tool_calls_by_index: BTreeMap<u64, OpenAiToolCallState> = BTreeMap::new();
    let mut finish_reason_raw = Value::Null;
    let mut usage_raw = Value::Null;
    let mut emitted_text = String::new();

    for obj in chunks {
        let Some(choice0) = obj
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(Value::as_object)
        else {
            continue;
        };
        let choice = Value::Object(choice0.clone());
        let text_snapshot = extract_choice_text(&choice);
        let (text_delta, next_emitted_text) = next_text_delta(&text_snapshot, &emitted_text);
        if !text_delta.is_empty() {
            events.push(ProviderStreamEvent::TextDelta { text: text_delta });
        }
        emitted_text = next_emitted_text;

        if let Some(tool_calls) = choice0
            .get("delta")
            .and_then(Value::as_object)
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(Value::as_array)
        {
            for tool_call in tool_calls {
                let Some(tool_call_obj) = tool_call.as_object() else {
                    continue;
                };
                let idx = value_as_u64(tool_call_obj.get("index")).unwrap_or(0);
                let record = tool_calls_by_index.entry(idx).or_default();
                if let Some(id) = tool_call_obj.get("id").and_then(Value::as_str) {
                    record.id = Some(id.to_string());
                }
                if let Some(function) = tool_call_obj.get("function").and_then(Value::as_object) {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        record.name = Some(name.to_string());
                    }
                    if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                        let (arguments_delta, arguments_emitted) =
                            next_text_delta(arguments, &record.arguments_emitted);
                        if !arguments_delta.is_empty() {
                            record.arguments.push_str(&arguments_delta);
                        }
                        record.arguments_emitted = arguments_emitted;
                    }
                }
            }
        }

        if let Some(finish_reason) = choice0.get("finish_reason")
            && !finish_reason.is_null()
        {
            finish_reason_raw = finish_reason.clone();
        }
        if let Some(usage) = obj.get("usage").filter(|value| value.is_object()) {
            usage_raw = usage.clone();
        }
    }

    let has_tool_calls = !tool_calls_by_index.is_empty();
    for (idx, record) in tool_calls_by_index {
        let call_id = record.id.unwrap_or_else(|| format!("openai_call_{idx}"));
        let name = record.name.unwrap_or_default();
        let input = parse_tool_arguments(&Value::String(record.arguments));
        events.push(ProviderStreamEvent::ToolCall {
            call_id,
            name,
            input,
        });
    }
    events.push(ProviderStreamEvent::Finish {
        finish_reason: map_openai_finish_reason(&finish_reason_raw, has_tool_calls).to_string(),
        usage: usage_from_openai(usage_raw.as_object()),
    });
    events
}

#[must_use]
pub fn parse_tool_arguments(arguments: &Value) -> Value {
    match arguments {
        Value::Object(_) => arguments.clone(),
        Value::Array(_) => json!({ "_value": arguments }),
        Value::String(raw) => {
            let raw_arguments = raw.trim();
            if raw_arguments.is_empty() {
                return json!({});
            }
            match serde_json::from_str::<Value>(raw_arguments) {
                Ok(Value::Object(item)) => Value::Object(item),
                Ok(value) => json!({ "_value": value }),
                Err(_) => match best_effort_load_json(raw_arguments) {
                    Some(Value::Object(item)) => Value::Object(item),
                    Some(value) => json!({ "_value": value }),
                    None => json!({ "_raw": raw }),
                },
            }
        }
        _ => json!({}),
    }
}

#[must_use]
pub fn summarize_http_error_body(raw: &str, content_type: &str) -> String {
    let text = raw;
    let lower_type = content_type.to_ascii_lowercase();
    let stripped = text.trim_start();
    let stripped_lower = stripped.to_ascii_lowercase();
    let looks_like_html = lower_type.contains("text/html")
        || stripped_lower.starts_with("<!doctype html")
        || stripped_lower.starts_with("<html");
    if looks_like_html {
        let title = extract_html_title(text);
        let suffix = if title.is_empty() {
            String::new()
        } else {
            format!(": {title}")
        };
        return format!("upstream returned HTML error page{suffix}");
    }
    let compact = compact_error_text(text);
    if compact.is_empty() {
        return "empty response body".to_string();
    }
    truncate_error_text(&compact)
}
