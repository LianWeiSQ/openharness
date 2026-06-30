use super::catalog::provider_catalog_summary;
use super::*;

pub(super) fn load_cached_provider_models_from_cache(
    cache: &Value,
    provider: &str,
) -> Option<Vec<Value>> {
    for key in provider_lookup_keys(provider) {
        if let Some(models) = cache
            .get("providers")
            .and_then(|providers| providers.get(&key))
            .and_then(|provider| provider.get("models"))
            .and_then(Value::as_array)
        {
            return Some(models.clone());
        }
        if let Some(models) = cache
            .get(&key)
            .and_then(|provider| provider.get("models"))
            .and_then(Value::as_object)
        {
            return Some(
                models
                    .iter()
                    .map(|(id, model)| {
                        let mut value = model.clone();
                        if let Some(object) = value.as_object_mut() {
                            object.entry("id".to_string()).or_insert_with(|| json!(id));
                        }
                        value
                    })
                    .collect(),
            );
        }
    }
    None
}

pub(super) fn provider_catalog_record(cache: &Value, provider: &str) -> Option<Value> {
    provider_lookup_keys(provider).into_iter().find_map(|key| {
        cache
            .get("providers")
            .and_then(|providers| providers.get(&key))
            .cloned()
            .map(|record| {
                let mut summary = provider_catalog_summary(&record);
                if let Some(object) = summary.as_object_mut() {
                    object.insert("selected".to_string(), json!(key == provider));
                }
                summary
            })
    })
}

pub(super) fn fallback_provider_catalog_record(provider: &str, model_count: usize) -> Value {
    json!({
        "id": provider,
        "label": provider_label(provider).unwrap_or_else(|_| provider.to_string()),
        "name": provider_label(provider).unwrap_or_else(|_| provider.to_string()),
        "default_model": provider_default_model(provider).ok().flatten(),
        "default_base_url": provider_default_base_url(provider).ok().flatten(),
        "requires_api_key": provider_requires_api_key(provider).unwrap_or(true),
        "native_streaming": provider_native_streaming(provider),
        "model_count": model_count,
        "source": "fallback",
    })
}

pub(super) fn provider_lookup_keys(provider: &str) -> Vec<String> {
    let normalized = normalize_models_provider_id(provider);
    let mut keys = vec![normalized.clone()];
    if normalized == "gemini" {
        keys.push("google".to_string());
    }
    keys.dedup();
    keys
}

pub(super) fn normalize_models_provider_id(provider: &str) -> String {
    match provider {
        "google" => "gemini".to_string(),
        other => normalize_provider(Some(other)).unwrap_or_else(|_| other.to_string()),
    }
}

pub(super) fn strip_provider_prefix(provider: &str, model_id: &str) -> String {
    for prefix in provider_lookup_keys(provider) {
        if let Some(value) = model_id.strip_prefix(&format!("{prefix}/")) {
            return value.to_string();
        }
    }
    if provider == "gemini"
        && let Some(value) = model_id.strip_prefix("google/")
    {
        return value.to_string();
    }
    model_id.to_string()
}

pub(super) fn provider_native_streaming(provider: &str) -> Value {
    json!({
        "chat_completions_sse": provider != "anthropic",
        "responses_sse": provider != "anthropic",
        "anthropic_messages_sse": provider == "anthropic",
        "implemented": matches!(provider, "anthropic" | "openai" | "openrouter" | "groq" | "mistral" | "deepseek" | "xai" | "ollama" | "gemini" | "azure-openai"),
    })
}

pub(super) fn compact_capabilities(capabilities: &Value) -> String {
    let mut parts = Vec::new();
    for key in [
        "vision",
        "tools",
        "streaming",
        "reasoning",
        "structured_output",
    ] {
        if capabilities
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            parts.push(key);
        }
    }
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(",")
    }
}
