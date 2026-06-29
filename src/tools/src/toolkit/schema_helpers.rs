fn tool_available(tool: &ToolDefinition, execution_mode: &str) -> bool {
    if execution_mode != "opensandbox" {
        return true;
    }
    matches!(
        tool.execution_scope,
        ToolExecutionScope::Workspace | ToolExecutionScope::Agnostic
    )
}

fn schema(required: &[&str], properties: &[&str]) -> Value {
    let props = properties
        .iter()
        .map(|name| ((*name).to_string(), property_schema(name)))
        .collect::<serde_json::Map<_, _>>();
    json!({
        "type": "object",
        "properties": props,
        "required": required,
    })
}

fn property_schema(name: &str) -> Value {
    match name {
        "offset" | "limit" | "timeout" => json!({"type": "integer"}),
        "replace_all" | "multiple" | "include_content" | "include_diagnostics" => {
            json!({"type": "boolean"})
        }
        "ignore" | "questions" | "todos" | "options" => json!({"type": "array"}),
        "value" => json!({}),
        _ => json!({"type": "string"}),
    }
}
