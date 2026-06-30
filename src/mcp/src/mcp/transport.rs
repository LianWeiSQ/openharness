#[must_use]
pub fn transport_candidates(transport: McpTransport) -> Vec<McpTransport> {
    match transport {
        McpTransport::Http => vec![McpTransport::Http],
        McpTransport::Sse => vec![McpTransport::Sse],
        McpTransport::Stdio => vec![McpTransport::Stdio],
        McpTransport::Auto => vec![McpTransport::Http, McpTransport::Sse],
    }
}

#[must_use]
pub fn dynamic_tool_name(server_name: &str, tool_name: &str) -> String {
    format!(
        "mcp_tool_{}_{}",
        sanitize_name(server_name),
        sanitize_name(tool_name)
    )
}

#[must_use]
pub fn sanitize_name(value: &str) -> String {
    let mut result = String::new();
    for item in value.trim().chars().flat_map(char::to_lowercase) {
        if item.is_ascii_alphanumeric() {
            result.push(item);
        } else if !result.ends_with('_') {
            result.push('_');
        }
    }
    let trimmed = result.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "tool".to_string()
    } else {
        trimmed
    }
}

#[must_use]
pub fn timeout_seconds(timeout_ms: u64) -> f64 {
    (timeout_ms as f64 / 1_000.0).max(1.0)
}
