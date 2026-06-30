fn runtime_agent_profile_from_value(
    value: &Value,
    fallback_id: &str,
    source_path: Option<PathBuf>,
) -> Option<RuntimeSubagentProfile> {
    if value.as_object().is_none_or(Map::is_empty) {
        return None;
    }
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("primary")
        .trim()
        .to_ascii_lowercase();
    if !matches!(mode.as_str(), "primary" | "subagent" | "all") {
        return None;
    }
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(sanitize_runtime_agent_id)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| sanitize_runtime_agent_id(fallback_id));
    let permission = runtime_profile_permission_ruleset_value(value)
        .and_then(|raw| parse_permission_ruleset(raw).ok())
        .unwrap_or(PermissionRuleset::PlanOnly);
    Some(RuntimeSubagentProfile {
        id: if id.is_empty() {
            "agent".to_string()
        } else {
            id
        },
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(fallback_id)
            .to_string(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        mode,
        permission,
        task_permissions: runtime_profile_task_permissions(value),
        prompt: value
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or(BUILD_AGENT_PROMPT)
            .trim_start_matches('\u{feff}')
            .to_string(),
        tools: runtime_profile_string_list(value.get("tools")),
        provider: value
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string),
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
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
        model_options: runtime_profile_model_options(value),
        hidden: value
            .get("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        source_path,
    })
}

fn runtime_profile_string_list(value: Option<&Value>) -> Vec<String> {
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

fn runtime_profile_model_options(value: &Value) -> BTreeMap<String, Value> {
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

fn sanitize_runtime_agent_id(value: &str) -> String {
    let mut output = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    while output.contains("--") {
        output = output.replace("--", "-");
    }
    if output.is_empty() {
        "agent".to_string()
    } else {
        output
    }
}
