use super::cache::ensure_models_cache;
use super::catalog::{models_catalog_payload, models_catalog_text};
use super::providers::{
    compact_capabilities, fallback_provider_catalog_record, load_cached_provider_models_from_cache,
    provider_catalog_record,
};
use super::*;

pub(crate) fn models_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(models_help());
    }
    let format = value_for(args, &["--format"]).unwrap_or_else(|| "table".to_string());
    let verbose = has_flag(args, &["--verbose"]);
    let catalog_requested = has_flag(args, &["--catalog", "--providers"]);
    let cache = ensure_models_cache(args);
    let provider = value_for(args, &["--provider"]).or_else(|| {
        positional_args(
            args,
            &[
                "--format",
                "--ttl-seconds",
                "--models-url",
                "--model",
                "-m",
                "--provider",
            ],
        )
        .first()
        .cloned()
    });
    if catalog_requested {
        let payload = models_catalog_payload(&cache);
        if format == "json" {
            return CliRunResult::ok_json(&payload);
        }
        return ok_text(models_catalog_text(&payload, verbose));
    }
    let provider = provider.unwrap_or_else(active_provider);
    let normalized = match normalize_provider(Some(&provider)) {
        Ok(provider) => provider,
        Err(error) => return err_text(2, error),
    };
    let cached = load_cached_provider_models_from_cache(&cache.value, &normalized);
    let models = if let Some(models) = cached.filter(|items| !items.is_empty()) {
        models
    } else {
        let model_id = value_for(args, &["--model", "-m"])
            .or_else(|| provider_env_value(&normalized, "model"))
            .unwrap_or_else(|| default_model_for_provider(&normalized));
        vec![if normalized == "anthropic" {
            serde_json::to_value(anthropic_model(&model_id, 200_000, 8192))
                .unwrap_or_else(|_| json!({}))
        } else {
            serde_json::to_value(openai_compatible_model(&normalized, &model_id))
                .unwrap_or_else(|_| json!({}))
        }]
    };
    let provider_info = provider_catalog_record(&cache.value, &normalized)
        .unwrap_or_else(|| fallback_provider_catalog_record(&normalized, models.len()));
    let payload = json!({
        "provider": normalized,
        "provider_label": provider_label(&provider).unwrap_or(provider),
        "provider_info": provider_info,
        "models": models,
        "cache": cache.to_value(),
        "cache_path": models_cache_path().to_string_lossy(),
        "refreshed": cache.refreshed,
        "stale": cache.stale,
        "fallback": cache.fallback,
    });
    if format == "json" {
        CliRunResult::ok_json(&payload)
    } else {
        let rows = payload["models"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|model| {
                let id = model.get("id").and_then(Value::as_str).unwrap_or("-");
                let context = model
                    .get("context_window")
                    .or_else(|| model.get("limit").and_then(|limit| limit.get("context")))
                    .and_then(Value::as_u64)
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string());
                let output = model
                    .get("max_output")
                    .or_else(|| model.get("limit").and_then(|limit| limit.get("output")))
                    .and_then(Value::as_u64)
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string());
                let capabilities = model
                    .get("capabilities")
                    .map(compact_capabilities)
                    .unwrap_or_else(|| "-".to_string());
                if verbose {
                    vec![
                        id.to_string(),
                        context,
                        output,
                        capabilities,
                        stable_json_dumps(model),
                    ]
                } else {
                    vec![id.to_string(), context, output, capabilities]
                }
            })
            .collect::<Vec<_>>();
        let provider_id = payload["provider"].as_str().unwrap_or("openai");
        let provider_label = payload["provider_label"].as_str().unwrap_or(provider_id);
        let mut text = render_key_values(
            "Models",
            &[
                ("Provider", format!("{provider_label} ({provider_id})")),
                ("Models", rows.len().to_string()),
                (
                    "Cache",
                    format!("{} ({})", cache.status, cache.path.to_string_lossy()),
                ),
            ],
        );
        if !rows.is_empty() {
            text.push_str("\n\n");
            text.push_str(&render_table(
                if verbose {
                    &["Model", "Context", "Output", "Capabilities", "Raw"]
                } else {
                    &["Model", "Context", "Output", "Capabilities"]
                },
                &rows,
            ));
        }
        ok_text(text)
    }
}
