fn provider_turn_result(
    store: &FileSessionStore,
    session: &Session,
    payload: &Value,
    stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
) -> Result<RuntimeProviderResult, String> {
    let provider_raw = payload
        .get("provider")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            session
                .metadata
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| std::env::var("OPENAGENT_PROVIDER").ok())
        .unwrap_or_else(|| "openai".to_string());
    let provider = normalize_provider(Some(&provider_raw))?;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            session
                .metadata
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| std::env::var("OPENAGENT_MODEL").ok())
        .or_else(|| provider_default_model(&provider).ok().flatten())
        .unwrap_or_else(|| "gpt-4o-mini".to_string());
    let env = default_env_mapping(&provider)?;
    let api_key = payload
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| env.get("api_key").and_then(|key| std::env::var(key).ok()))
        .or_else(|| std::env::var("OPENAGENT_API_KEY").ok());
    if provider_requires_api_key(&provider)? && api_key.as_deref().unwrap_or_default().is_empty() {
        return Ok(RuntimeProviderResult {
            answer: format!(
                "Provider `{provider}` is not configured. Set {} or OPENAGENT_API_KEY, then retry this turn.",
                env.get("api_key")
                    .map(String::as_str)
                    .unwrap_or("OPENAI_API_KEY")
            ),
            tool_calls: Vec::new(),
            usage: Usage::default(),
            source: "provider_missing_api_key".to_string(),
            finish_reason: "configuration_required".to_string(),
        });
    }
    let api_key = api_key.unwrap_or_default();
    let base_url = payload
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| env.get("base_url").and_then(|key| std::env::var(key).ok()))
        .or_else(|| provider_default_base_url(&provider).ok().flatten())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let wire_api = payload
        .get("wire_api")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| env.get("wire_api").and_then(|key| std::env::var(key).ok()))
        .unwrap_or_else(|| "responses".to_string());
    let timeout = payload
        .get("timeout_s")
        .and_then(Value::as_u64)
        .unwrap_or(60);
    let stream = provider_streaming_enabled_for_turn(payload);
    let tools = Toolkit::with_builtins().get_all_tools("local");
    let provider_messages = store
        .materialized_chat_messages(session)
        .unwrap_or_else(|_| session.messages.clone());
    call_openai_compatible_provider_for_runtime(
        OpenAiRuntimeProviderRequest {
            provider: &provider,
            model: &model,
            api_key: &api_key,
            base_url: &base_url,
            wire_api: &wire_api,
            timeout_s: timeout,
            stream,
            messages: &provider_messages,
            tools: &tools,
        },
        stream_sink,
    )
}

fn call_openai_compatible_provider_for_runtime(
    request: OpenAiRuntimeProviderRequest<'_>,
    mut stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
) -> Result<RuntimeProviderResult, String> {
    let OpenAiRuntimeProviderRequest {
        provider,
        model,
        api_key,
        base_url,
        wire_api,
        timeout_s,
        stream,
        messages,
        tools,
    } = request;
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(timeout_s.max(1)))
        .build()
        .map_err(|error| error.to_string())?;
    let mut config = OpenAiLanguageModelConfig::new(api_key, model);
    config.provider_id = provider.to_string();
    config.base_url = base_url.to_string();
    config.wire_api = wire_api.to_string();
    let (endpoint, payload) = if wire_api == "chat" {
        let mut payload =
            build_openai_chat_payload(&config, None, messages, tools, None, None, None);
        if let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(stream));
        }
        (join_url(base_url, "chat/completions"), payload)
    } else {
        let mut payload =
            build_openai_responses_payload(&config, None, messages, tools, None, None);
        if let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(stream));
        }
        (join_url(base_url, "responses"), payload)
    };
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
    if stream && content_type.contains("text/event-stream") {
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
            if let Some(event) = openai_stream_text_delta(wire_api, &chunk)
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
        return Ok(provider_events_to_runtime_result(
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
        Ok(openai_chat_response_to_runtime_result(
            &value,
            format!("{provider}:chat"),
        ))
    } else {
        let events = normalize_openai_responses_response(&value);
        Ok(provider_events_to_runtime_result(
            &events,
            format!("{provider}:responses"),
            Some(&value),
        ))
    }
}
