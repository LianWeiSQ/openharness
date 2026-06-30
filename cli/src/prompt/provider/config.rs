fn apply_agent_model_options_to_payload(payload: &mut Value, profile: Option<&RunAgentProfile>) {
    let Some(profile) = profile else {
        return;
    };
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    for (key, value) in &profile.model_options {
        if provider_payload_option_allowed(key) {
            object.insert(key.clone(), value.clone());
        }
    }
    if let Some(temperature) = profile.temperature {
        object.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = profile.top_p {
        object.insert("top_p".to_string(), json!(top_p));
    }
}

fn provider_payload_option_allowed(key: &str) -> bool {
    !matches!(
        key,
        "model" | "messages" | "input" | "tools" | "tool_choice" | "stream"
    )
}

fn provider_streaming_enabled(args: &[String]) -> bool {
    has_flag(args, &["--stream"])
        || env::var("OPENAGENT_STREAM").is_ok_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

fn mock_tool_calls_from_env() -> Result<Option<Vec<ToolCall>>, String> {
    let Some(raw) = env::var("OPENAGENT_MOCK_TOOL_CALLS")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("OPENAGENT_MOCK_TOOL_CALLS is not JSON: {error}"))?;
    let items = if let Some(items) = value.as_array() {
        items.clone()
    } else {
        vec![value]
    };
    let mut calls = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "mock tool call requires name".to_string())?;
        let call_id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map_or_else(|| format!("mock_call_{index}"), str::to_string);
        let input = item
            .get("input")
            .or_else(|| item.get("arguments"))
            .map(parse_tool_arguments)
            .unwrap_or_else(|| json!({}));
        calls.push(ToolCall {
            name: name.to_string(),
            input,
            call_id,
        });
    }
    Ok(Some(calls))
}

fn provider_api_key(provider: &str, args: &[String]) -> Option<String> {
    value_for(args, &["--api-key"]).or_else(|| provider_env_value(provider, "api_key"))
}

fn provider_base_url(provider: &str, args: &[String]) -> String {
    value_for(args, &["--base-url"])
        .or_else(|| provider_env_value(provider, "base_url"))
        .or_else(|| provider_default_base_url(provider).ok().flatten())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn is_synthetic_endpoint(base_url: &str) -> bool {
    base_url.contains(".test")
        || base_url.contains("example.com")
        || base_url.contains("example/v1")
        || base_url.contains("localhost:0")
}

fn provider_wire_api(provider: &str, args: &[String]) -> String {
    value_for(args, &["--wire-api"])
        .or_else(|| provider_env_value(provider, "wire_api"))
        .unwrap_or_else(|| {
            if provider == "anthropic" {
                "messages".to_string()
            } else {
                DEFAULT_WIRE_API.to_string()
            }
        })
}
