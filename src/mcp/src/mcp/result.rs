#[must_use]
pub fn normalize_tool_call_result(
    descriptor: &RemoteMcpToolDescriptor,
    transport: Option<McpTransport>,
    result: &Value,
) -> RemoteMcpToolCallResult {
    let (mut output, non_text_blocks) = render_tool_result_output(result);
    let is_error = result.get("isError").is_some_and(legacy_truthy);
    let metadata = build_result_metadata(descriptor, transport, is_error, &non_text_blocks);
    let mut error = None;
    if is_error {
        error = Some(if output.is_empty() {
            "Remote MCP tool returned an error.".to_string()
        } else {
            output
        });
        output = String::new();
    } else if output.is_empty() {
        output = "(Remote MCP tool completed with no textual output.)".to_string();
    }
    RemoteMcpToolCallResult {
        output,
        error,
        metadata,
    }
}

#[must_use]
pub fn unavailable_tool_result(dynamic_name: &str) -> RemoteMcpToolCallResult {
    RemoteMcpToolCallResult {
        output: String::new(),
        error: Some(format!(
            "Remote MCP tool '{dynamic_name}' is not available."
        )),
        metadata: BTreeMap::from([
            ("tool".to_string(), Value::String(dynamic_name.to_string())),
            ("backend".to_string(), Value::String("mcp".to_string())),
            (
                "mcp_tool_name".to_string(),
                Value::String(dynamic_name.to_string()),
            ),
        ]),
    }
}

#[must_use]
pub fn render_tool_result_output(result: &Value) -> (String, Vec<String>) {
    let mut parts = Vec::new();
    let mut non_text_blocks = Vec::new();
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in content {
        let item_type = item
            .get("type")
            .filter(|value| legacy_truthy(value))
            .map_or_else(String::new, value_to_legacy_string)
            .trim()
            .to_ascii_lowercase();
        if item_type == "text" {
            let text = item
                .get("text")
                .filter(|value| legacy_truthy(value))
                .map_or_else(String::new, value_to_legacy_string)
                .trim()
                .to_string();
            if !text.is_empty() {
                parts.push(text);
            }
            continue;
        }
        let kind = non_text_block_kind(&item_type);
        non_text_blocks.push(kind.clone());
        parts.push(format!("[MCP content ignored: {kind}]"));
    }
    (
        parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        dedupe_preserve_order(&non_text_blocks),
    )
}

#[must_use]
pub fn build_result_metadata(
    descriptor: &RemoteMcpToolDescriptor,
    transport: Option<McpTransport>,
    is_error: bool,
    non_text_blocks: &[String],
) -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "tool".to_string(),
            Value::String(descriptor.dynamic_name.clone()),
        ),
        (
            "title".to_string(),
            Value::String(result_title(
                &descriptor.server_name,
                &descriptor.original_name,
            )),
        ),
        ("backend".to_string(), Value::String("mcp".to_string())),
        (
            "mcp_server".to_string(),
            Value::String(descriptor.server_name.clone()),
        ),
        (
            "mcp_original_tool_name".to_string(),
            Value::String(descriptor.original_name.clone()),
        ),
        (
            "mcp_transport".to_string(),
            transport.map_or(Value::Null, |item| Value::String(item.as_str().to_string())),
        ),
        (
            "mcp_tool_name".to_string(),
            Value::String(descriptor.dynamic_name.clone()),
        ),
        ("mcp_non_text_blocks".to_string(), json!(non_text_blocks)),
        ("is_error".to_string(), Value::Bool(is_error)),
    ])
}

#[must_use]
pub fn mcp_tool_definition(descriptor: &RemoteMcpToolDescriptor, group: &str) -> ToolDefinition {
    ToolDefinition {
        id: descriptor.dynamic_name.clone(),
        description: descriptor.description.clone(),
        parameter_schema: descriptor.input_schema.clone(),
        dangerous: true,
        group: group.to_string(),
        execution_scope: ToolExecutionScope::Agnostic,
        execution_schema: ToolExecutionSchema {
            read_only: false,
            mutates_workspace: false,
            mutates_session: false,
            mutates_external: false,
            external_io: true,
            requires_user_interaction: false,
            concurrency: ToolConcurrency::Unknown,
            batch_group: group.to_string(),
            conflict_key_template: None,
            max_parallelism: None,
        },
    }
}

#[must_use]
pub fn bridge_tool_output(
    descriptor: &RemoteMcpToolDescriptor,
    result: RemoteMcpToolCallResult,
) -> McpBridgeOutput {
    let title = result_title(&descriptor.server_name, &descriptor.original_name);
    let mut metadata = result.metadata;
    metadata
        .entry("tool".to_string())
        .or_insert_with(|| Value::String(descriptor.dynamic_name.clone()));
    metadata
        .entry("title".to_string())
        .or_insert_with(|| Value::String(title.clone()));
    metadata
        .entry("backend".to_string())
        .or_insert_with(|| Value::String("mcp".to_string()));
    metadata
        .entry("mcp_server".to_string())
        .or_insert_with(|| Value::String(descriptor.server_name.clone()));
    metadata
        .entry("mcp_original_tool_name".to_string())
        .or_insert_with(|| Value::String(descriptor.original_name.clone()));
    metadata
        .entry("mcp_transport".to_string())
        .or_insert(Value::Null);
    metadata
        .entry("mcp_tool_name".to_string())
        .or_insert_with(|| Value::String(descriptor.dynamic_name.clone()));
    metadata
        .entry("mcp_non_text_blocks".to_string())
        .or_insert_with(|| json!([]));
    McpBridgeOutput {
        title,
        output: result.output,
        metadata,
        truncated: false,
        attachments: Vec::new(),
        error: result.error,
    }
}
