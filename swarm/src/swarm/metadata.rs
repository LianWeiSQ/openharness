fn run_status_from_str(value: &str) -> RunStatus {
    match value {
        "partial" => RunStatus::Partial,
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        _ => RunStatus::Completed,
    }
}

fn status_str(result: &AgentResult) -> &'static str {
    match result.status {
        RunStatus::Completed => "completed",
        RunStatus::Partial => "partial",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(item)) => vec![item.clone()],
        _ => Vec::new(),
    }
}

fn artifacts_from_value(value: Option<&Value>) -> Vec<ArtifactRef> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| serde_json::from_value::<ArtifactRef>(item.clone()).ok())
            .collect(),
        _ => Vec::new(),
    }
}

fn usage_from_value(value: Option<&Value>) -> SwarmUsage {
    match value {
        Some(value) => serde_json::from_value::<SwarmUsage>(value.clone()).unwrap_or_default(),
        None => SwarmUsage::default(),
    }
}

fn map_from_value(value: Option<&Value>) -> BTreeMap<String, Value> {
    match value {
        Some(Value::Object(items)) => items.clone().into_iter().collect(),
        _ => BTreeMap::new(),
    }
}

fn command_from_metadata(metadata: &BTreeMap<String, Value>) -> SwarmResult<SubprocessCommand> {
    let raw = metadata
        .get("command")
        .or_else(|| metadata.get("argv"))
        .ok_or_else(|| "subprocess runner metadata.command is required".to_string())?;
    let argv = match raw {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        Value::String(command) => command
            .split_whitespace()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let env = metadata
        .get("env")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    Ok(SubprocessCommand {
        argv,
        cwd: metadata
            .get("cwd")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        env,
        timeout_seconds: metadata.get("timeout_seconds").and_then(Value::as_f64),
    })
}

fn http_request_from_metadata(
    metadata: &BTreeMap<String, Value>,
) -> SwarmResult<HttpRequestConfig> {
    let url = metadata
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "http runner metadata.url is required".to_string())?
        .to_string();
    Ok(HttpRequestConfig {
        url,
        method: metadata
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("POST")
            .to_uppercase(),
        headers: string_map(metadata.get("headers")),
        timeout_seconds: metadata.get("timeout_seconds").and_then(Value::as_f64),
    })
}

fn a2a_request_from_metadata(metadata: &BTreeMap<String, Value>) -> SwarmResult<A2ARequestConfig> {
    let url = metadata
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "a2a runner metadata.url is required".to_string())?
        .to_string();
    Ok(A2ARequestConfig {
        url,
        headers: string_map(metadata.get("headers")),
        timeout_seconds: metadata.get("timeout_seconds").and_then(Value::as_f64),
    })
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default()
}

fn message_send_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with("/message/send") || trimmed.ends_with("/message/stream") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/message/send")
    }
}

fn stable_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
}

fn generated_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("swarm_{nanos}")
}

fn default_post() -> String {
    "POST".to_string()
}

fn default_function_kind() -> String {
    "function".to_string()
}

fn default_model_tier() -> String {
    "worker".to_string()
}

fn default_permission_mode() -> PermissionMode {
    PermissionMode::Readonly
}
