use serde_json::{Value, json};

pub(crate) fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

pub(crate) fn usage_totals_value(
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    cost: f64,
) -> Value {
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens,
        "cost": cost,
    })
}

pub(crate) fn trim_lines(value: &str, max_lines: usize) -> Vec<String> {
    let lines = value.lines().map(ToString::to_string).collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return lines;
    }
    let omitted = lines.len() - max_lines;
    let mut output = lines.into_iter().take(max_lines).collect::<Vec<_>>();
    output.push(format!("... diff truncated ({omitted} more lines) ..."));
    output
}

pub(crate) fn clip_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    format!("{}...", value.chars().take(limit).collect::<String>())
}

pub(crate) fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn object_value(value: Option<&Value>) -> Value {
    value
        .and_then(Value::as_object)
        .cloned()
        .map(Value::Object)
        .unwrap_or_else(|| json!({}))
}

pub(crate) trait IfEmptyThen {
    fn if_empty_then<F>(self, fallback: F) -> String
    where
        F: FnOnce() -> String;
}

impl IfEmptyThen for String {
    fn if_empty_then<F>(self, fallback: F) -> String
    where
        F: FnOnce() -> String,
    {
        if self.is_empty() { fallback() } else { self }
    }
}
