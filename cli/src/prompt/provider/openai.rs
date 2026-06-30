fn call_openai_compatible_provider(
    args: &[String],
    provider: &str,
    api_key: &str,
    model_id: &str,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    mut stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
    agent_profile: Option<&RunAgentProfile>,
) -> Result<ProviderRunResult, String> {
    let base_url = provider_base_url(provider, args);
    if is_synthetic_endpoint(&base_url) {
        return Ok(ProviderRunResult {
            answer: "hello from openagent".to_string(),
            tool_calls: Vec::new(),
            usage: Usage::default(),
            source: "offline_fallback_synthetic_endpoint".to_string(),
            finish_reason: "stop".to_string(),
        });
    }
    let wire_api = provider_wire_api(provider, args);
    let timeout = Duration::from_secs(
        value_for(args, &["--timeout-s"])
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(60),
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())?;
    let mut config = OpenAiLanguageModelConfig::new(api_key, model_id);
    config.provider_id = provider.to_string();
    config.base_url = base_url.clone();
    config.wire_api = wire_api.clone();
    config.reasoning_effort = value_for(args, &["--variant"]);
    let stream = provider_streaming_enabled(args);
    let (endpoint, mut payload) = if wire_api == "chat" {
        let mut payload =
            build_openai_chat_payload(&config, None, messages, tools, None, None, None);
        if let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(stream));
        }
        (join_url(&base_url, "chat/completions"), payload)
    } else {
        let mut payload =
            build_openai_responses_payload(&config, None, messages, tools, None, None);
        if stream && let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(true));
        }
        (join_url(&base_url, "responses"), payload)
    };
    apply_agent_model_options_to_payload(&mut payload, agent_profile);
    if let Some(max_tokens) =
        value_for(args, &["--max-output-tokens"]).and_then(|value| value.parse::<u64>().ok())
        && let Some(object) = payload.as_object_mut()
    {
        object.insert(
            if wire_api == "chat" {
                "max_tokens"
            } else {
                "max_output_tokens"
            }
            .to_string(),
            json!(max_tokens),
        );
    }
    let mut request = client
        .post(endpoint)
        .bearer_auth(api_key)
        .header("content-type", "application/json");
    if stream {
        request = request.header("accept", "text/event-stream");
    }
    let response = request
        .json(&payload)
        .send()
        .map_err(|error| format!("provider request failed: {error}"))?;
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
                .map_err(|error| format!("provider response read failed: {error}"))?;
            return Err(format!(
                "provider returned HTTP {}: {}",
                status.as_u16(),
                summarize_http_error_body(&raw, &content_type)
            ));
        }
        let mut chunks = Vec::new();
        read_sse_json_values_stream(response, |chunk| {
            if let Some(event) = openai_stream_text_delta(&wire_api, &chunk)
                && let Some(sink) = stream_sink.as_deref_mut()
            {
                sink(&event);
            }
            chunks.push(chunk);
            Ok(())
        })?;
        let events = if wire_api == "chat" {
            normalize_openai_chat_sse_chunks(&chunks)
        } else {
            normalize_openai_responses_stream_events(&chunks)
        };
        return Ok(provider_events_to_run_result(
            &events,
            format!("{provider}:{wire_api}:stream"),
            None,
        ));
    }
    let raw = response
        .text()
        .map_err(|error| format!("provider response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "provider returned HTTP {}: {}",
            status.as_u16(),
            summarize_http_error_body(&raw, &content_type)
        ));
    }
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("provider response was not JSON: {error}"))?;
    if wire_api == "chat" {
        let answer = extract_chat_answer(&value);
        let tool_calls = extract_chat_tool_calls(&value);
        let finish_reason = extract_chat_finish_reason(&value).unwrap_or_else(|| {
            if tool_calls.is_empty() {
                "stop"
            } else {
                "tool_call"
            }
            .to_string()
        });
        Ok(ProviderRunResult {
            answer: if answer.is_empty() && tool_calls.is_empty() {
                stable_json_dumps(&value)
            } else {
                answer
            },
            tool_calls,
            usage: usage_from_json(value.get("usage")),
            source: format!("{provider}:{wire_api}"),
            finish_reason,
        })
    } else {
        let events = normalize_openai_responses_response(&value);
        Ok(provider_events_to_run_result(
            &events,
            format!("{provider}:{wire_api}"),
            Some(&value),
        ))
    }
}
