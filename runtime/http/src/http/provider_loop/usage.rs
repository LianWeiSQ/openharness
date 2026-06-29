fn usage_from_provider_json(value: Option<&Value>) -> Usage {
    let input_tokens = value
        .and_then(|item| {
            item.get("input_tokens")
                .or_else(|| item.get("prompt_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or_default();
    let output_tokens = value
        .and_then(|item| {
            item.get("output_tokens")
                .or_else(|| item.get("completion_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or_default();
    Usage {
        input_tokens,
        output_tokens,
        cost: 0.0,
    }
}

fn usage_value_from_provider(
    usage: &Usage,
    tool_calls: u64,
    fallback_input: &str,
    fallback_output: &str,
) -> Value {
    let fallback = usage_payload(fallback_input, fallback_output, tool_calls);
    let input_tokens = if usage.input_tokens == 0 {
        fallback["input_tokens"].as_u64().unwrap_or_default()
    } else {
        usage.input_tokens
    };
    let output_tokens = if usage.output_tokens == 0 {
        fallback["output_tokens"].as_u64().unwrap_or_default()
    } else {
        usage.output_tokens
    };
    let tool_tokens = tool_calls.saturating_mul(16);
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "tool_tokens": tool_tokens,
        "total_tokens": input_tokens + output_tokens + tool_tokens,
        "tool_calls": tool_calls,
        "cost": usage.cost,
        "estimated": usage.input_tokens == 0 && usage.output_tokens == 0,
    })
}

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}
