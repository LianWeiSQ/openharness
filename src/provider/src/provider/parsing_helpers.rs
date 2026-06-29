fn best_effort_load_json(raw_text: &str) -> Option<Value> {
    let mut best_candidate = None;
    let mut best_score: Option<(u8, usize, std::cmp::Reverse<usize>)> = None;
    for (index, char_value) in raw_text.char_indices() {
        if char_value != '{' && char_value != '[' {
            continue;
        }
        let slice = &raw_text[index..];
        let mut stream = serde_json::Deserializer::from_str(slice).into_iter::<Value>();
        let Some(Ok(candidate)) = stream.next() else {
            continue;
        };
        let end_index = index + stream.byte_offset();
        let trailing = raw_text[end_index..].trim();
        let score = (
            u8::from(trailing.is_empty()),
            end_index - index,
            std::cmp::Reverse(index),
        );
        if best_score.is_none_or(|current| score > current) {
            best_candidate = Some(candidate);
            best_score = Some(score);
            if score.0 == 1 {
                break;
            }
        }
    }
    best_candidate
}

fn extract_text_content(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(extract_text_content)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(""),
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                return text.to_string();
            }
            if let Some(text_value) = map.get("text")
                && let Some(nested) = text_value.get("value").and_then(Value::as_str)
            {
                return nested.to_string();
            }
            if let Some(delta) = map.get("delta") {
                if let Some(delta) = delta.as_str() {
                    return delta.to_string();
                }
                if matches!(delta, Value::Object(_) | Value::Array(_)) {
                    let nested_delta = extract_text_content(delta);
                    if !nested_delta.is_empty() {
                        return nested_delta;
                    }
                }
            }
            for key in ["content", "value", "output_text"] {
                if let Some(nested) = map.get(key)
                    && matches!(
                        nested,
                        Value::String(_) | Value::Object(_) | Value::Array(_)
                    )
                {
                    let extracted = extract_text_content(nested);
                    if !extracted.is_empty() {
                        return extracted;
                    }
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

fn extract_choice_text(choice: &Value) -> String {
    if let Some(delta) = choice.get("delta").filter(|value| value.is_object()) {
        let extracted = extract_text_content(delta);
        if !extracted.is_empty() {
            return extracted;
        }
    }
    if let Some(message) = choice.get("message").filter(|value| value.is_object()) {
        let extracted = extract_text_content(message);
        if !extracted.is_empty() {
            return extracted;
        }
    }
    String::new()
}

fn next_text_delta(raw_text: &str, emitted_text: &str) -> (String, String) {
    if raw_text.is_empty() {
        return (String::new(), emitted_text.to_string());
    }
    if let Some(delta) = raw_text.strip_prefix(emitted_text) {
        return (delta.to_string(), raw_text.to_string());
    }
    if emitted_text.starts_with(raw_text) {
        return (String::new(), emitted_text.to_string());
    }
    (raw_text.to_string(), format!("{emitted_text}{raw_text}"))
}

fn usage_from_openai(usage: Option<&Map<String, Value>>) -> Usage {
    Usage {
        input_tokens: usage
            .and_then(|item| item.get("prompt_tokens"))
            .and_then(value_to_u64)
            .unwrap_or(0),
        output_tokens: usage
            .and_then(|item| item.get("completion_tokens"))
            .and_then(value_to_u64)
            .unwrap_or(0),
        cost: 0.0,
    }
}

fn usage_from_responses(usage: Option<&Map<String, Value>>) -> Usage {
    let input_tokens = usage
        .and_then(|item| {
            item.get("input_tokens")
                .or_else(|| item.get("prompt_tokens"))
                .and_then(value_to_u64)
        })
        .unwrap_or(0);
    let output_tokens = usage
        .and_then(|item| {
            item.get("output_tokens")
                .or_else(|| item.get("completion_tokens"))
                .and_then(value_to_u64)
        })
        .unwrap_or(0);
    Usage {
        input_tokens,
        output_tokens,
        cost: 0.0,
    }
}

fn map_openai_finish_reason(value: &Value, has_tool_calls: bool) -> &'static str {
    if let Some(reason) = value.as_str() {
        if reason == "stop" {
            return "stop";
        }
        if reason == "length" {
            return "length";
        }
        if reason == "tool_calls" || reason == "tool_call" {
            return "tool_call";
        }
    }
    if has_tool_calls {
        "tool_call"
    } else {
        "unknown"
    }
}

fn map_anthropic_finish_reason(value: &Value, has_tool_calls: bool) -> &'static str {
    let reason = value.as_str().unwrap_or_default().trim();
    match reason {
        "tool_use" => "tool_call",
        "end_turn" | "stop_sequence" | "stop" => "stop",
        "max_tokens" => "length",
        _ if has_tool_calls => "tool_call",
        _ => "unknown",
    }
}

fn compact_error_text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_error_text(text: &str) -> String {
    if text.chars().count() <= HTTP_ERROR_BODY_PREVIEW_CHARS {
        return text.to_string();
    }
    let truncated = text
        .chars()
        .take(HTTP_ERROR_BODY_PREVIEW_CHARS)
        .collect::<String>();
    format!("{}...", truncated.trim_end())
}

fn extract_html_title(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let Some(start_tag) = lower.find("<title") else {
        return String::new();
    };
    let Some(start_close) = lower[start_tag..].find('>') else {
        return String::new();
    };
    let content_start = start_tag + start_close + 1;
    let Some(end_tag) = lower[content_start..].find("</title>") else {
        return String::new();
    };
    let title = &raw[content_start..content_start + end_tag];
    truncate_error_text(&compact_error_text(&html_unescape_minimal(title)))
}

fn html_unescape_minimal(raw: &str) -> String {
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}
