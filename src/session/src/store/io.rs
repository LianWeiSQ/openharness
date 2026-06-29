fn write_json(path: &Path, payload: &impl Serialize) -> SessionResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|item| item.to_str())
            .unwrap_or("json")
    ));
    fs::write(&tmp, serde_json::to_string_pretty(payload)? + "\n")?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn append_jsonl(path: &Path, payload: &impl Serialize) -> SessionResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(serde_json::to_string(payload)?.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

fn read_json_object(path: &Path) -> SessionResult<Option<Map<String, Value>>> {
    if !path.exists() {
        return Ok(None);
    }
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
    Ok(value.as_object().cloned())
}

fn read_jsonl(path: &Path) -> SessionResult<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).map_err(Into::into))
        .collect()
}

fn next_seq(path: &Path) -> SessionResult<u64> {
    Ok(read_jsonl(path)?.len() as u64 + 1)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", now_ms())
}

fn bool_option(value: &Value, default: bool) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::String(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "on" | "true" | "yes")
                || (!matches!(normalized.as_str(), "0" | "false" | "no" | "off") && default)
        }
        Value::Null => default,
        _ => value.as_bool().unwrap_or(default),
    }
}

fn sanitize_value_map(map: BTreeMap<String, Value>, max_chars: usize) -> BTreeMap<String, Value> {
    map.into_iter()
        .map(|(key, value)| {
            if is_sensitive_key(&key) && !SAFE_TOKEN_METRIC_KEYS.contains(&key.as_str()) {
                (key, json!("[redacted]"))
            } else {
                (key, sanitize_value(value, max_chars))
            }
        })
        .collect()
}

fn sanitize_value(value: Value, max_chars: usize) -> Value {
    match value {
        Value::Object(items) => Value::Object(
            items
                .into_iter()
                .map(|(key, value)| {
                    if is_sensitive_key(&key) && !SAFE_TOKEN_METRIC_KEYS.contains(&key.as_str()) {
                        (key, json!("[redacted]"))
                    } else {
                        (key, sanitize_value(value, max_chars))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| sanitize_value(item, max_chars))
                .collect(),
        ),
        Value::String(value) => Value::String(truncate_text(&value, max_chars)),
        other => other,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SENSITIVE_KEY_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let len = value.chars().count();
    if max_chars == 0 {
        return String::new();
    }
    if len <= max_chars {
        return value.to_string();
    }
    let hidden = len.saturating_sub(max_chars.saturating_sub(24));
    let suffix = format!("...[truncated {hidden} chars]");
    let prefix_len = max_chars.saturating_sub(suffix.chars().count());
    format!(
        "{}{}",
        value.chars().take(prefix_len).collect::<String>(),
        suffix
    )
}

fn stable_json_dumps(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => {
            if *value {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(stable_json_dumps)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(items) => format!(
            "{{{}}}",
            items
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    serde_json::to_string(key).unwrap_or_default(),
                    stable_json_dumps(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn normalize_level(level: &str) -> String {
    match level.to_ascii_uppercase().as_str() {
        "DEBUG" | "INFO" | "WARNING" | "ERROR" | "CRITICAL" => level.to_ascii_uppercase(),
        _ => "INFO".to_string(),
    }
}

fn level_number(level: &str) -> u8 {
    match normalize_level(level).as_str() {
        "DEBUG" => 10,
        "INFO" => 20,
        "WARNING" => 30,
        "ERROR" => 40,
        "CRITICAL" => 50,
        _ => 20,
    }
}

fn metrics_with_threshold(
    base: &BTreeMap<String, Value>,
    threshold: u64,
) -> BTreeMap<String, Value> {
    let mut metrics = base.clone();
    metrics.insert("threshold".to_string(), json!(threshold));
    metrics
}

fn warning_title(code: &str) -> String {
    match code {
        "context_usage_high" => "Context usage high".to_string(),
        "context_usage_critical" => "Context usage critical".to_string(),
        "step_input_tokens_exceeded" => "Step input token budget exceeded".to_string(),
        "step_output_tokens_exceeded" => "Step output token budget exceeded".to_string(),
        "step_total_tokens_exceeded" => "Step token budget exceeded".to_string(),
        "step_cost_exceeded" => "Step cost budget exceeded".to_string(),
        other => title_case(&other.replace('_', " ")),
    }
}

fn title_case(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_metrics(code: &str, metrics: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    let keys = if code.starts_with("context_usage_") {
        vec![
            "step_index",
            "usage_ratio",
            "threshold",
            "estimated_input_tokens",
            "input_limit_tokens",
            "fallback_stage",
        ]
    } else if code == "step_cost_exceeded" {
        vec![
            "step_index",
            "cost",
            "threshold",
            "input_tokens",
            "output_tokens",
            "total_tokens",
        ]
    } else if code.starts_with("step_") {
        vec![
            "step_index",
            "input_tokens",
            "output_tokens",
            "total_tokens",
            "threshold",
        ]
    } else {
        return metrics.clone();
    };
    keys.into_iter()
        .filter_map(|key| {
            metrics
                .get(key)
                .filter(|value| !value.is_null())
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect()
}

fn format_display_metrics(metrics: &Map<String, Value>) -> String {
    metrics
        .iter()
        .map(|(key, value)| {
            let text = if let Some(number) = value.as_f64() {
                if (key.ends_with("ratio") || key == "threshold") && number > 0.0 && number <= 1.0 {
                    format!("{:.1}%", number * 100.0)
                } else {
                    format_float(number)
                }
            } else {
                value
                    .as_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| value.to_string())
            };
            format!("{key}={text}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_float(value: f64) -> String {
    let rendered = format!("{value:.6}");
    rendered
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}
