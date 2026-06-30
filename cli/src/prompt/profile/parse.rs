fn agent_profile_from_value(
    value: &Value,
    source_path: Option<PathBuf>,
    fallback_id: &str,
    fallback_name: &str,
    loaded: bool,
) -> Result<RunAgentProfile, String> {
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("primary")
        .to_ascii_lowercase();
    if !matches!(mode.as_str(), "primary" | "subagent" | "all") {
        return Err(format!(
            "agent profile {fallback_id} has invalid mode '{mode}'; expected primary, subagent, or all"
        ));
    }
    Ok(RunAgentProfile {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .map(sanitize_identifier)
            .unwrap_or_else(|| sanitize_identifier(fallback_id)),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(fallback_name)
            .to_string(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        mode,
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        provider: value
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string),
        permission: profile_permission_ruleset_value(value).map(str::to_string),
        task_permissions: profile_task_permissions(value),
        prompt: value
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::to_string),
        tools: profile_string_list(value.get("tools")),
        max_steps: value
            .get("max_steps")
            .or_else(|| value.get("steps"))
            .or_else(|| value.get("maxSteps"))
            .and_then(Value::as_u64),
        temperature: value.get("temperature").and_then(Value::as_f64),
        top_p: value
            .get("top_p")
            .or_else(|| value.get("topP"))
            .and_then(Value::as_f64),
        color: value
            .get("color")
            .and_then(Value::as_str)
            .map(str::to_string),
        disabled: value
            .get("disabled")
            .or_else(|| value.get("disable"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        model_options: profile_model_options(value),
        hidden: value
            .get("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        source_path,
        loaded,
    })
}

fn profile_string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        Some(Value::String(item)) => item
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn profile_model_options(value: &Value) -> BTreeMap<String, Value> {
    const KNOWN_KEYS: &[&str] = &[
        "id",
        "name",
        "description",
        "mode",
        "model",
        "provider",
        "permission",
        "task",
        "task_permissions",
        "tools",
        "prompt",
        "steps",
        "max_steps",
        "maxSteps",
        "hidden",
        "color",
        "disabled",
        "disable",
        "temperature",
        "top_p",
        "topP",
        "model_options",
        "options",
        "source_path",
        "loaded",
    ];
    let mut options = BTreeMap::new();
    if let Some(object) = value.as_object() {
        for (key, item) in object {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                options.insert(key.clone(), item.clone());
            }
        }
    }
    if let Some(temperature) = value.get("temperature").and_then(Value::as_f64) {
        options.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = value
        .get("top_p")
        .or_else(|| value.get("topP"))
        .and_then(Value::as_f64)
    {
        options.insert("top_p".to_string(), json!(top_p));
    }
    for key in ["model_options", "options"] {
        if let Some(object) = value.get(key).and_then(Value::as_object) {
            for (option_key, option_value) in object {
                options.insert(option_key.clone(), option_value.clone());
            }
        }
    }
    options
}

pub(crate) fn agent_profile_public_value(profile: &RunAgentProfile) -> Value {
    json!({
        "id": profile.id.clone(),
        "name": profile.name.clone(),
        "description": profile.description.clone(),
        "mode": profile.mode.clone(),
        "model": profile.model.clone(),
        "provider": profile.provider.clone(),
        "permission": profile.permission.clone(),
        "task_permissions": profile.task_permissions.clone(),
        "tools": profile.tools.clone(),
        "max_steps": profile.max_steps,
        "steps": profile.max_steps,
        "temperature": profile.temperature,
        "top_p": profile.top_p,
        "color": profile.color.clone(),
        "disabled": profile.disabled,
        "model_options": profile.model_options.clone(),
        "hidden": profile.hidden,
        "loaded": profile.loaded,
        "source_path": profile.source_path.as_ref().map(|path| path.to_string_lossy().to_string()),
    })
}
