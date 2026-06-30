fn call_anthropic_provider(
    args: &[String],
    api_key: &str,
    model_id: &str,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    mut stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
) -> Result<ProviderRunResult, String> {
    let timeout = Duration::from_secs(60);
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())?;
    let mut config = AnthropicLanguageModelConfig::new(api_key, model_id);
    config.base_url =
        value_for(args, &["--base-url"]).or_else(|| provider_env_value("anthropic", "base_url"));
    let stream = provider_streaming_enabled(args);
    let mut payload = build_anthropic_payload(&config, None, messages, tools, None, None, None);
    if let Some(object) = payload.as_object_mut() {
        object.insert("stream".to_string(), json!(stream));
    }
    let endpoint = join_url(
        config
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com/v1"),
        "messages",
    );
    let mut request = client
        .post(endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json");
    if stream {
        request = request.header("accept", "text/event-stream");
    }
    let response = request
        .json(&payload)
        .send()
        .map_err(|error| format!("anthropic request failed: {error}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if stream {
        if !status.is_success() {
            let raw = response
                .text()
                .map_err(|error| format!("anthropic response read failed: {error}"))?;
            return Err(format!(
                "anthropic returned HTTP {}: {}",
                status.as_u16(),
                summarize_http_error_body(&raw, &content_type)
            ));
        }
        let mut chunks = Vec::new();
        read_sse_json_values_stream(response, |chunk| {
            if let Some(event) = anthropic_stream_text_delta(&chunk)
                && let Some(sink) = stream_sink.as_deref_mut()
            {
                sink(&event);
            }
            chunks.push(chunk);
            Ok(())
        })?;
        let events = normalize_anthropic_events(&chunks);
        return Ok(provider_events_to_run_result(
            &events,
            "anthropic:messages:stream".to_string(),
            None,
        ));
    }
    let raw = response
        .text()
        .map_err(|error| format!("anthropic response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "anthropic returned HTTP {}: {}",
            status.as_u16(),
            summarize_http_error_body(&raw, &content_type)
        ));
    }
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("anthropic response was not JSON: {error}"))?;
    let answer = value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    let tool_calls = extract_anthropic_tool_calls(&value);
    Ok(ProviderRunResult {
        answer,
        tool_calls,
        usage: usage_from_json(value.get("usage")),
        source: "anthropic:messages".to_string(),
        finish_reason: value
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("stop")
            .to_string(),
    })
}
