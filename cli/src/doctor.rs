use super::*;

pub(super) fn doctor_payload_from_args(provider: &str, args: &[String]) -> Value {
    let mut payload = doctor_payload_from_env(provider);
    let Ok(normalized) = normalize_provider(Some(provider)) else {
        return payload;
    };
    let Some(object) = payload.as_object_mut() else {
        return payload;
    };
    if normalized == "anthropic" {
        if let Some(api_key) = value_for(args, &["--api-key"])
            && !api_key.is_empty()
        {
            object.insert("api_key_set".to_string(), json!(true));
            object.insert(
                "healthy".to_string(),
                json!(bool_field(object, "dependency_ok")),
            );
            object.insert(
                "model_endpoint_ok".to_string(),
                json!(bool_field(object, "dependency_ok")),
            );
        }
        if let Some(base_url) = value_for(args, &["--base-url"]) {
            object.insert("base_url".to_string(), json!(base_url));
        }
        if let Some(model) = value_for(args, &["--model", "-m"]) {
            object.insert("model".to_string(), json!(model));
        }
        if let Some(wire_api) = value_for(args, &["--wire-api"]) {
            object.insert("wire_api".to_string(), json!(wire_api));
        }
        return payload;
    }

    if let Some(api_key) = value_for(args, &["--api-key"])
        && !api_key.is_empty()
    {
        object.insert("api_key_set".to_string(), json!(true));
    }
    if let Some(base_url) = value_for(args, &["--base-url"]) {
        object.insert("base_url".to_string(), json!(base_url));
    }
    if let Some(model) = value_for(args, &["--model", "-m"]) {
        object.insert("model".to_string(), json!(model));
    }
    if let Some(wire_api) = value_for(args, &["--wire-api"]) {
        object.insert("wire_api".to_string(), json!(wire_api));
    }
    payload
}

fn doctor_payload_from_env(provider: &str) -> Value {
    if provider == "anthropic" {
        return json!({
            "provider": "anthropic",
            "provider_label": "Anthropic",
            "base_url": env::var("ANTHROPIC_BASE_URL").ok(),
            "model": env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string()),
            "wire_api": "messages",
            "api_key_env": "ANTHROPIC_API_KEY",
            "api_key_set": env::var("ANTHROPIC_API_KEY").is_ok_and(|value| !value.is_empty()),
            "native": true,
            "healthy": env::var("ANTHROPIC_API_KEY").is_ok_and(|value| !value.is_empty()),
            "dependency_checked": true,
            "dependency_ok": true,
            "dependency_message": "optional dependency 'anthropic' is installed",
            "model_endpoint_checked": false,
            "model_endpoint_ok": env::var("ANTHROPIC_API_KEY").is_ok_and(|value| !value.is_empty()),
            "model_endpoint_message": "skipped OpenAI-compatible /models probe for native provider",
        });
    }
    let endpoint_ok = env::var("OPENAGENT_DOCTOR_MODEL_ENDPOINT_OK")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes"));
    json!({
        "provider": "openai",
        "provider_label": "OpenAI",
        "base_url": env::var("OPENAI_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()),
        "model": env::var("OPENAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
        "wire_api": env::var("OPENAI_WIRE_API").unwrap_or_else(|_| DEFAULT_WIRE_API.to_string()),
        "api_key_env": "OPENAI_API_KEY",
        "api_key_set": env::var("OPENAI_API_KEY").is_ok_and(|value| !value.is_empty()),
        "native": false,
        "healthy": endpoint_ok,
        "dependency_checked": false,
        "dependency_ok": true,
        "dependency_message": null,
        "model_endpoint_checked": true,
        "model_endpoint_ok": endpoint_ok,
        "model_endpoint_message": env::var("OPENAGENT_DOCTOR_MODEL_ENDPOINT_MESSAGE").unwrap_or_else(|_| "not checked by Rust CLI smoke".to_string()),
    })
}

pub(super) fn doctor_text_from_payload(payload: &Value) -> String {
    let object = payload.as_object().expect("doctor payload object");
    let healthy = bool_field(object, "healthy");
    let api_key = if bool_field(object, "api_key_set") {
        "set"
    } else {
        "missing"
    };
    if object
        .get("native")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut text = render_key_values(
            "OpenAgent Doctor",
            &[
                (
                    "Status",
                    if healthy { "ok" } else { "needs attention" }.to_string(),
                ),
                (
                    "Provider",
                    format!(
                        "{} ({})",
                        string_field(object, "provider_label"),
                        string_field(object, "provider")
                    ),
                ),
                ("Model", string_field(object, "model")),
                (
                    "API Key",
                    format!("{api_key} ({})", string_field(object, "api_key_env")),
                ),
                ("Base URL", string_field(object, "base_url")),
            ],
        );
        text.push_str("\n\n");
        text.push_str(&render_table(
            &["Check", "Status", "Detail"],
            &[
                vec![
                    "Dependency".to_string(),
                    if bool_field(object, "dependency_ok") {
                        "ok".to_string()
                    } else {
                        "missing".to_string()
                    },
                    string_field(object, "dependency_message"),
                ],
                vec![
                    "Model Endpoint".to_string(),
                    "skipped".to_string(),
                    string_field(object, "model_endpoint_message"),
                ],
            ],
        ));
        text.push('\n');
        return text;
    }
    let mut text = render_key_values(
        "OpenAgent Doctor",
        &[
            (
                "Status",
                if healthy { "ok" } else { "needs attention" }.to_string(),
            ),
            (
                "Provider",
                format!(
                    "{} ({})",
                    string_field(object, "provider_label"),
                    string_field(object, "provider")
                ),
            ),
            ("Model", string_field(object, "model")),
            ("Wire API", string_field(object, "wire_api")),
            (
                "API Key",
                format!("{api_key} ({})", string_field(object, "api_key_env")),
            ),
            ("Base URL", string_field(object, "base_url")),
        ],
    );
    text.push_str("\n\n");
    text.push_str(&render_table(
        &["Check", "Status", "Detail"],
        &[vec![
            "Model Endpoint".to_string(),
            if bool_field(object, "model_endpoint_ok") {
                "ok".to_string()
            } else {
                "failed".to_string()
            },
            string_field(object, "model_endpoint_message"),
        ]],
    ));
    text.push('\n');
    text
}

fn string_field(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn bool_field(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).and_then(Value::as_bool).unwrap_or(false)
}
