fn step_index_from_metadata(metadata: &BTreeMap<String, Value>) -> Option<u64> {
    metadata
        .get("step_index")
        .or_else(|| metadata.get("step"))
        .and_then(Value::as_u64)
}

fn session_status_str(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::Paused => "paused",
        SessionStatus::Stop => "stop",
        SessionStatus::Compacting => "compacting",
    }
}

fn session_status_from_value(value: Option<&Value>) -> SessionStatus {
    match value.and_then(Value::as_str) {
        Some("running") => SessionStatus::Running,
        Some("paused") => SessionStatus::Paused,
        Some("stop") => SessionStatus::Stop,
        Some("compacting") => SessionStatus::Compacting,
        _ => SessionStatus::Idle,
    }
}

fn count_events(events: &[Value], name: &str) -> u64 {
    events
        .iter()
        .filter(|event| event.get("event").and_then(Value::as_str) == Some(name))
        .count() as u64
}

fn count_by_key(rows: &[Value], key: &str) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for row in rows {
        if let Some(value) = row.get(key).and_then(Value::as_str) {
            *counts.entry(value.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

fn tool_source(attrs: &Map<String, Value>) -> &'static str {
    if let Some(source) = attrs
        .get("tool_source")
        .or_else(|| attrs.get("source"))
        .and_then(Value::as_str)
    {
        if !source.is_empty() {
            return match source {
                "mcp" => "mcp",
                "skill" => "skill",
                "local" => "local",
                "local_tool" => "local_tool",
                _ => "unknown",
            };
        }
    }
    if attrs.get("backend").and_then(Value::as_str) == Some("mcp") {
        return "mcp";
    }
    if attrs.get("skill_name").is_some()
        || attrs.get("tool_group").and_then(Value::as_str) == Some("skill")
    {
        return "skill";
    }
    if attrs.get("tool_group").and_then(Value::as_str).is_some() {
        return "local_tool";
    }
    "unknown"
}

fn inc_summary_u64(summary: &mut Map<String, Value>, key: &str, amount: u64) {
    let current = summary.get(key).and_then(Value::as_u64).unwrap_or_default();
    summary.insert(key.to_string(), json!(current + amount));
}

fn inc_summary_f64(summary: &mut Map<String, Value>, key: &str, amount: f64) {
    let current = summary.get(key).and_then(Value::as_f64).unwrap_or_default();
    summary.insert(key.to_string(), json!(current + amount));
}

fn u64_field(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
