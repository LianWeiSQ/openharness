fn normalize_regression_thresholds(raw: Option<&Value>) -> BTreeMap<String, f64> {
    let allowed = [
        "max_cost_delta",
        "max_duration_delta_ms",
        "max_input_tokens_delta",
        "max_output_tokens_delta",
        "max_total_tokens_delta",
        "max_tool_calls_delta",
        "max_model_calls_delta",
    ];
    let Some(object) = raw.and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    allowed
        .into_iter()
        .filter_map(|key| {
            object
                .get(key)
                .and_then(Value::as_f64)
                .map(|value| (key.to_string(), value))
        })
        .collect()
}

fn budget_regressions(deltas: &[(&str, f64)], thresholds: &BTreeMap<String, f64>) -> Vec<String> {
    let checks = [
        ("max_cost_delta", "cost_delta"),
        ("max_duration_delta_ms", "duration_delta_ms"),
        ("max_input_tokens_delta", "input_tokens_delta"),
        ("max_output_tokens_delta", "output_tokens_delta"),
        ("max_total_tokens_delta", "total_tokens_delta"),
        ("max_tool_calls_delta", "tool_calls_delta"),
        ("max_model_calls_delta", "model_calls_delta"),
    ];
    checks
        .into_iter()
        .filter_map(|(threshold_key, delta_key)| {
            let threshold = thresholds.get(threshold_key)?;
            let delta = deltas
                .iter()
                .find_map(|(key, value)| (*key == delta_key).then_some(*value))
                .unwrap_or(0.0);
            (delta > *threshold).then(|| {
                format!(
                    "{delta_key} exceeded {threshold_key}: {} > {}",
                    format_g(delta),
                    format_g(*threshold)
                )
            })
        })
        .collect()
}

fn case_regression_fields(item: &Value) -> Value {
    let fields = [
        "status",
        "score",
        "duration_ms",
        "steps",
        "model_calls",
        "tool_calls",
        "input_tokens",
        "output_tokens",
        "cost",
        "trace_check_ok",
        "runtime_warning_count",
    ];
    let mut result = Map::new();
    for field in fields {
        if let Some(value) = item.get(field) {
            result.insert(field.to_string(), value.clone());
        }
    }
    Value::Object(result)
}

fn average(values: impl Iterator<Item = f64>) -> f64 {
    let materialized = values.collect::<Vec<_>>();
    if materialized.is_empty() {
        0.0
    } else {
        materialized.iter().sum::<f64>() / materialized.len() as f64
    }
}

fn compare_f64(left: f64, right: f64) -> Ordering {
    left.partial_cmp(&right).unwrap_or(Ordering::Equal)
}

fn computed_success_rate(results: &[Value]) -> f64 {
    if results.is_empty() {
        return 0.0;
    }
    let passed = results
        .iter()
        .filter(|item| item.get("status").and_then(Value::as_str) == Some("pass"))
        .count();
    passed as f64 / results.len() as f64
}

fn count_trace_check_failed(results: &[Value]) -> i64 {
    results
        .iter()
        .filter(|item| {
            !item
                .get("trace_check_ok")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .count() as i64
}

fn sum_result_int(results: &[Value], key: &str) -> i64 {
    results.iter().map(|item| int_field(item, key)).sum()
}

fn int_value(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|item| i64::try_from(item).ok()))
        .or_else(|| value.as_f64().map(|item| item as i64))
        .unwrap_or(0)
}

fn float_value(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

fn int_field(item: &Value, field: &str) -> i64 {
    item.get(field).map_or(0, int_value)
}

fn float_field(item: &Value, field: &str) -> f64 {
    item.get(field).map_or(0.0, float_value)
}

fn string_field(item: &Value, field: &str) -> String {
    item.get(field)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn bool_field(item: &Value, field: &str) -> bool {
    item.get(field).and_then(Value::as_bool).unwrap_or(false)
}

fn format_g(value: f64) -> String {
    if (value - value.round()).abs() < 1e-9 {
        return format!("{}", value.round() as i64);
    }
    let mut text = format!("{value:.6}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_@%+=:,./-".contains(ch))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn fixture_path(name: &str) -> String {
    format!("{FIXTURE_ROOT}/{name}")
}
