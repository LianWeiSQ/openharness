use super::cache::{ModelsCacheLoad, empty_models_cache};
use super::providers::{
    normalize_models_provider_id, provider_native_streaming, strip_provider_prefix,
};
use super::*;

pub(super) fn normalize_models_catalog(value: &Value, source_url: &str, ttl_seconds: u64) -> Value {
    if value
        .get("schema_version")
        .and_then(Value::as_str)
        .is_some_and(|schema| schema == "openagent.models_cache.v1")
    {
        return value.clone();
    }
    let mut providers = Map::new();
    let mut catalog = Vec::new();
    let mut model_count = 0_usize;
    let Some(source_providers) = value.as_object() else {
        return empty_models_cache(ttl_seconds);
    };
    for (raw_provider_id, provider_value) in source_providers {
        let Some(provider_object) = provider_value.as_object() else {
            continue;
        };
        let provider_id = normalize_models_provider_id(raw_provider_id);
        let upstream_id = provider_object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(raw_provider_id);
        let models = provider_object
            .get("models")
            .and_then(Value::as_object)
            .map(|items| {
                items
                    .iter()
                    .map(|(model_id, model)| {
                        normalize_models_dev_model(&provider_id, model_id, model)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let provider_model_count = models.len();
        model_count += provider_model_count;
        let label = provider_label(&provider_id).unwrap_or_else(|_| {
            provider_object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&provider_id)
                .to_string()
        });
        let record = json!({
            "id": provider_id,
            "source_id": upstream_id,
            "name": provider_object.get("name").and_then(Value::as_str).unwrap_or(label.as_str()),
            "label": label,
            "api": provider_object.get("api").cloned().unwrap_or(Value::Null),
            "doc": provider_object.get("doc").cloned().unwrap_or(Value::Null),
            "npm": provider_object.get("npm").cloned().unwrap_or(Value::Null),
            "env": provider_object.get("env").cloned().unwrap_or_else(|| json!([])),
            "default_model": provider_default_model(&provider_id).ok().flatten(),
            "default_base_url": provider_default_base_url(&provider_id).ok().flatten(),
            "requires_api_key": provider_requires_api_key(&provider_id).unwrap_or(true),
            "native_streaming": provider_native_streaming(&provider_id),
            "model_count": provider_model_count,
            "models": models,
        });
        catalog.push(provider_catalog_summary(&record));
        providers.insert(provider_id, record);
    }
    catalog.sort_by(|left, right| {
        left.get("id")
            .and_then(Value::as_str)
            .cmp(&right.get("id").and_then(Value::as_str))
    });
    json!({
        "schema_version": "openagent.models_cache.v1",
        "source": {
            "url": source_url,
            "fetched_at_ms": now_ms_cli(),
            "ttl_seconds": ttl_seconds,
            "provider_count": providers.len(),
            "model_count": model_count,
            "raw_schema": "models.dev/api.json",
        },
        "providers": providers,
        "catalog": catalog,
    })
}

fn normalize_models_dev_model(provider_id: &str, model_id: &str, model: &Value) -> Value {
    let id = model
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(model_id)
        .to_string();
    let provider_model_id = strip_provider_prefix(provider_id, &id);
    let context_window = model
        .get("limit")
        .and_then(|limit| limit.get("context").or_else(|| limit.get("input")))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| openai_compatible_model(provider_id, &provider_model_id).context_window);
    let max_output = model
        .get("limit")
        .and_then(|limit| limit.get("output"))
        .and_then(Value::as_u64)
        .unwrap_or(4096);
    let input_modalities = model
        .get("modalities")
        .and_then(|modalities| modalities.get("input"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let vision = input_modalities.iter().any(|item| {
        item.as_str()
            .is_some_and(|value| matches!(value, "image" | "video" | "pdf"))
    }) || model
        .get("attachment")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    json!({
        "id": id,
        "provider_model_id": provider_model_id,
        "provider_id": provider_id,
        "name": model.get("name").and_then(Value::as_str).unwrap_or(model_id),
        "family": model.get("family").cloned().unwrap_or(Value::Null),
        "context_window": context_window,
        "max_output": max_output,
        "capabilities": {
            "vision": vision,
            "tools": model.get("tool_call").and_then(Value::as_bool).unwrap_or(true),
            "streaming": true,
            "reasoning": model.get("reasoning").and_then(Value::as_bool).unwrap_or(false),
            "structured_output": model.get("structured_output").and_then(Value::as_bool).unwrap_or(false),
            "temperature": model.get("temperature").and_then(Value::as_bool).unwrap_or(true),
        },
        "pricing": {
            "input_per_1m": model.get("cost").and_then(|cost| cost.get("input")).and_then(Value::as_f64).unwrap_or(0.0),
            "output_per_1m": model.get("cost").and_then(|cost| cost.get("output")).and_then(Value::as_f64).unwrap_or(0.0),
            "cache_read_per_1m": model.get("cost").and_then(|cost| cost.get("cache_read")).and_then(Value::as_f64).unwrap_or(0.0),
            "cache_write_per_1m": model.get("cost").and_then(|cost| cost.get("cache_write")).and_then(Value::as_f64).unwrap_or(0.0),
        },
        "modalities": model.get("modalities").cloned().unwrap_or(Value::Null),
        "knowledge": model.get("knowledge").cloned().unwrap_or(Value::Null),
        "release_date": model.get("release_date").cloned().unwrap_or(Value::Null),
        "last_updated": model.get("last_updated").cloned().unwrap_or(Value::Null),
        "open_weights": model.get("open_weights").cloned().unwrap_or(Value::Null),
        "raw": model,
    })
}

pub(super) fn models_catalog_payload(cache: &ModelsCacheLoad) -> Value {
    json!({
        "schema_version": "openagent.models_catalog.v1",
        "cache": cache.to_value(),
        "providers": cache.value.get("catalog").cloned().unwrap_or_else(|| json!([])),
    })
}

pub(super) fn models_catalog_text(payload: &Value, verbose: bool) -> String {
    let mut lines = vec![format!(
        "providers: {}",
        payload
            .get("providers")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    )];
    for provider in payload
        .get("providers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = provider.get("id").and_then(Value::as_str).unwrap_or("-");
        let label = provider.get("label").and_then(Value::as_str).unwrap_or(id);
        let count = provider
            .get("model_count")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if verbose {
            lines.push(format!(
                "{id} ({label}) models={count} {}",
                stable_json_dumps(provider)
            ));
        } else {
            lines.push(format!("{id} ({label}) models={count}"));
        }
    }
    lines.join("\n")
}

pub(super) fn provider_catalog_summary(provider: &Value) -> Value {
    json!({
        "id": provider.get("id").cloned().unwrap_or(Value::Null),
        "source_id": provider.get("source_id").cloned().unwrap_or(Value::Null),
        "name": provider.get("name").cloned().unwrap_or(Value::Null),
        "label": provider.get("label").cloned().unwrap_or(Value::Null),
        "api": provider.get("api").cloned().unwrap_or(Value::Null),
        "doc": provider.get("doc").cloned().unwrap_or(Value::Null),
        "npm": provider.get("npm").cloned().unwrap_or(Value::Null),
        "env": provider.get("env").cloned().unwrap_or_else(|| json!([])),
        "default_model": provider.get("default_model").cloned().unwrap_or(Value::Null),
        "default_base_url": provider.get("default_base_url").cloned().unwrap_or(Value::Null),
        "requires_api_key": provider.get("requires_api_key").cloned().unwrap_or(Value::Bool(true)),
        "native_streaming": provider.get("native_streaming").cloned().unwrap_or_else(|| json!({})),
        "model_count": provider.get("model_count").cloned().unwrap_or_else(|| json!(0)),
    })
}
