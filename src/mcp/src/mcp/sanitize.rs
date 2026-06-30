#[must_use]
pub fn sanitize_mcp_value(value: &Value, max_chars: usize) -> Value {
    match value {
        Value::Object(object) => {
            let mut sanitized = Map::new();
            for (key, item) in object {
                if is_sensitive_key(key) && !SAFE_TOKEN_METRIC_KEYS.contains(&key.as_str()) {
                    sanitized.insert(key.clone(), Value::String("[redacted]".to_string()));
                } else {
                    sanitized.insert(key.clone(), sanitize_mcp_value(item, max_chars));
                }
            }
            Value::Object(sanitized)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| sanitize_mcp_value(item, max_chars))
                .collect(),
        ),
        Value::String(text) => Value::String(truncate_text(text, max_chars)),
        _ => value.clone(),
    }
}

#[must_use]
pub fn sanitize_mcp_trace_value(value: &Value) -> Value {
    sanitize_mcp_value(value, 4096)
}

#[must_use]
pub fn sanitize_mcp_observation_value(value: &Value) -> Value {
    sanitize_mcp_value(value, 4096)
}
