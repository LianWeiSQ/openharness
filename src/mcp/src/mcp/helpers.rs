fn tool_description(server_name: &str, original_name: &str, description: &str) -> String {
    let base = format!(
        "Remote MCP tool from server '{server_name}'. Original MCP tool name: '{original_name}'."
    );
    let description = description.trim();
    if description.is_empty() {
        base
    } else {
        format!("{base}\n\n{description}")
    }
}

fn normalize_input_schema(raw: Option<&Value>) -> Value {
    let Some(Value::Object(object)) = raw else {
        return json!({"type": "object", "properties": {}});
    };
    if object.get("type") != Some(&Value::String("object".to_string())) {
        return json!({
            "type": "object",
            "properties": {},
            "x-mcp-original-schema": Value::Object(object.clone()),
        });
    }
    let mut schema = object.clone();
    schema
        .entry("type".to_string())
        .or_insert_with(|| Value::String("object".to_string()));
    schema
        .entry("properties".to_string())
        .or_insert_with(|| json!({}));
    Value::Object(schema)
}

fn result_title(server_name: &str, tool_name: &str) -> String {
    format!("MCP {server_name}/{tool_name}")
}

fn non_text_block_kind(item_type: &str) -> String {
    match item_type {
        "image" | "resource" => item_type.to_string(),
        "blob" | "binary" | "audio" | "video" => "binary".to_string(),
        _ => "unknown".to_string(),
    }
}

fn dedupe_preserve_order(values: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut ordered = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            ordered.push(value.clone());
        }
    }
    ordered
}

fn duplicate_suffix(server_name: &str, original_name: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{server_name}:{original_name}").as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    digest.chars().take(6).collect()
}

fn fnmatchcase(value: &str, pattern: &str) -> bool {
    let value_chars = value.chars().collect::<Vec<_>>();
    let pattern_chars = pattern.chars().collect::<Vec<_>>();
    let rows = pattern_chars.len() + 1;
    let cols = value_chars.len() + 1;
    let mut dp = vec![vec![false; cols]; rows];
    dp[0][0] = true;
    for i in 1..rows {
        if pattern_chars[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..rows {
        for j in 1..cols {
            let pattern_char = pattern_chars[i - 1];
            dp[i][j] = if pattern_char == '*' {
                dp[i - 1][j] || dp[i][j - 1]
            } else {
                (pattern_char == '?' || pattern_char == value_chars[j - 1]) && dp[i - 1][j - 1]
            };
        }
    }
    dp[rows - 1][cols - 1]
}

fn parse_int(value: Option<&Value>, default: u64, minimum: u64) -> u64 {
    let Some(value) = value else {
        return default.max(minimum);
    };
    let parsed = match value {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_i64()
                .and_then(|item| u64::try_from(item).ok())
                .or_else(|| number.as_f64().map(|item| item.trunc().max(0.0) as u64))
        }),
        Value::String(text) => text.parse::<u64>().ok(),
        Value::Bool(flag) => Some(u64::from(*flag)),
        _ => None,
    };
    parsed.unwrap_or(default).max(minimum)
}

fn parse_float(value: Option<&Value>, default: f64, minimum: f64) -> f64 {
    let Some(value) = value else {
        return default.max(minimum);
    };
    let parsed = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse::<f64>().ok(),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        _ => None,
    };
    parsed.unwrap_or(default).max(minimum)
}

fn value_to_legacy_string(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(flag) => {
            if *flag {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value)
            .unwrap_or_else(|error| format!("<json serialization error: {error}>")),
    }
}

fn legacy_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number
            .as_f64()
            .is_some_and(|item| item != 0.0 && !item.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SENSITIVE_KEY_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if max_chars == 0 {
        return String::new();
    }
    if char_count <= max_chars {
        return value.to_string();
    }
    let omitted = char_count.saturating_sub(max_chars.saturating_sub(24));
    let suffix = format!("...[truncated {omitted} chars]");
    let keep = max_chars.saturating_sub(suffix.chars().count());
    let prefix = value.chars().take(keep).collect::<String>();
    format!("{prefix}{suffix}")
}
