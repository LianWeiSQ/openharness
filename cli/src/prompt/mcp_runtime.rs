use super::*;

#[derive(Clone, Debug)]
pub(super) struct McpRuntime {
    pub(super) manager: RemoteMcpManager,
    pub(super) descriptors: BTreeMap<String, RemoteMcpToolDescriptor>,
    pub(super) snapshot: Value,
}

pub(super) fn load_mcp_runtime(
    args: &[String],
    toolkit: &mut Toolkit,
) -> Result<Option<McpRuntime>, String> {
    let Some(source) = mcp_runtime_source(args) else {
        return Ok(None);
    };
    let config = load_mcp_config(&source)?;
    if !config.enabled() {
        return Ok(Some(McpRuntime {
            manager: RemoteMcpManager::new(config),
            descriptors: BTreeMap::new(),
            snapshot: json!({}),
        }));
    }
    let mut manager = RemoteMcpManager::new(config.clone());
    let mut descriptors_by_name = BTreeMap::new();
    for server in config.servers.iter().filter(|server| server.enabled) {
        let (transport, tools) = discover_mcp_server_tools(server)?;
        let descriptors = build_tool_descriptors_from_values(server, &tools);
        for descriptor in &descriptors {
            toolkit
                .registry
                .register(mcp_tool_definition(descriptor, "remote-mcp"));
            descriptors_by_name.insert(descriptor.dynamic_name.clone(), descriptor.clone());
        }
        manager.set_server_tools(
            &server.name,
            Some(transport),
            "connected",
            Some(now_ms_cli() as f64 / 1000.0),
            descriptors,
        )?;
    }
    let snapshot = serde_json::to_value(manager.snapshot()).unwrap_or_else(|_| json!({}));
    Ok(Some(McpRuntime {
        manager,
        descriptors: descriptors_by_name,
        snapshot,
    }))
}

fn mcp_runtime_source(args: &[String]) -> Option<String> {
    value_for(args, &["--mcp-config"])
        .or_else(|| env::var("OPENAGENT_MCP_CONFIG").ok())
        .or_else(|| {
            let path = mcp_config_path(args);
            path.exists().then(|| path.to_string_lossy().to_string())
        })
}

pub(crate) fn discover_mcp_server_tools(
    server: &RemoteMcpServerConfig,
) -> Result<(McpTransport, Vec<Value>), String> {
    let mut errors = Vec::new();
    for transport in transport_candidates(server.transport) {
        match mcp_json_rpc(server, transport, "tools/list", json!({})) {
            Ok(value) => {
                let tools = value
                    .get("tools")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                return Ok((transport, tools));
            }
            Err(error) => errors.push(format!("{}: {error}", transport.as_str())),
        }
    }
    Err(format!(
        "MCP tools/list failed for server '{}': {}",
        server.name,
        errors.join("; ")
    ))
}

fn mcp_json_rpc(
    server: &RemoteMcpServerConfig,
    transport: McpTransport,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let timeout = Duration::from_millis(server.timeout_ms);
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client
        .post(&server.url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": format!("openagent-{}", now_ms_cli()),
            "method": method,
            "params": params,
        }));
    for (key, value) in &server.headers {
        request = request.header(key, value);
    }
    let response = request
        .send()
        .map_err(|error| format!("{} request failed: {error}", transport.as_str()))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response
        .text()
        .map_err(|error| format!("MCP response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            summarize_http_error_body(&raw, &content_type)
        ));
    }
    let value = if content_type.contains("text/event-stream") {
        parse_sse_json_values(&raw)?
            .into_iter()
            .find(|item| item.get("result").is_some() || item.get("error").is_some())
            .ok_or_else(|| "MCP SSE response did not contain a JSON-RPC result".to_string())?
    } else {
        serde_json::from_str::<Value>(&raw)
            .map_err(|error| format!("MCP response was not JSON: {error}"))?
    };
    if let Some(error) = value.get("error") {
        return Err(format!("MCP JSON-RPC error: {}", python_json_dumps(error)));
    }
    Ok(value.get("result").cloned().unwrap_or(value))
}

pub(super) fn execute_mcp_tool(
    mcp_runtime: Option<&McpRuntime>,
    tool_call: &ToolCall,
) -> Option<ToolResult> {
    let runtime = mcp_runtime?;
    let descriptor = runtime.descriptors.get(&tool_call.name)?;
    let Some(state) = runtime.manager.servers.get(&descriptor.server_name) else {
        let result = unavailable_tool_result(&tool_call.name);
        let bridge = bridge_tool_output(descriptor, result);
        return Some(mcp_bridge_to_tool_result(tool_call, bridge));
    };
    let transport = state.selected_transport.unwrap_or(McpTransport::Http);
    let result = match mcp_json_rpc(
        &state.config,
        transport,
        "tools/call",
        json!({
            "name": descriptor.original_name,
            "arguments": tool_call.input.clone(),
        }),
    ) {
        Ok(value) => normalize_tool_call_result(descriptor, Some(transport), &value),
        Err(error) => {
            let mut result = unavailable_tool_result(&tool_call.name);
            result.error = Some(error);
            result
        }
    };
    Some(mcp_bridge_to_tool_result(
        tool_call,
        bridge_tool_output(descriptor, result),
    ))
}

fn mcp_bridge_to_tool_result(
    tool_call: &ToolCall,
    bridge: openagent_mcp::McpBridgeOutput,
) -> ToolResult {
    ToolResult {
        call_id: tool_call.call_id.clone(),
        output: bridge.output,
        error: bridge.error,
        metadata: bridge.metadata,
    }
}
