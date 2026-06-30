fn string_field(payload: &Value, key: &str) -> String {
    payload
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn optional_string_field(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn summarize_http_error_body(raw: &str, content_type: &str) -> String {
    if raw.trim().is_empty() {
        return "empty response body".to_string();
    }
    if content_type.contains("json")
        && let Ok(value) = serde_json::from_str::<Value>(raw)
    {
        if let Some(error) = value
            .get("error")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return error.to_string();
        }
        return value.to_string();
    }
    raw.lines().take(5).collect::<Vec<_>>().join("\n")
}

trait EmptyStringExt {
    fn if_empty_then<F>(self, fallback: F) -> Self
    where
        F: FnOnce() -> Self;
}

impl EmptyStringExt for String {
    fn if_empty_then<F>(self, fallback: F) -> Self
    where
        F: FnOnce() -> Self,
    {
        if self.is_empty() { fallback() } else { self }
    }
}
