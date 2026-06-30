use super::*;
use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, ChildStdin, Command as ProcessCommand, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
};

#[derive(Clone, Debug)]
pub(super) struct McpRuntime {
    pub(super) manager: RemoteMcpManager,
    pub(super) descriptors: BTreeMap<String, RemoteMcpToolDescriptor>,
    pub(super) snapshot: Value,
    pub(super) workspace: PathBuf,
}

pub(super) fn load_mcp_runtime(
    args: &[String],
    toolkit: &mut Toolkit,
) -> Result<Option<McpRuntime>, String> {
    let Some(source) = mcp_runtime_source(args) else {
        return Ok(None);
    };
    let config = load_mcp_config(&source)?;
    let workspace = workspace_from_args(args);
    if !config.enabled() {
        return Ok(Some(McpRuntime {
            manager: RemoteMcpManager::new(config),
            descriptors: BTreeMap::new(),
            snapshot: json!({}),
            workspace,
        }));
    }
    let mut manager = RemoteMcpManager::new(config.clone());
    let mut descriptors_by_name = BTreeMap::new();
    for server in config.servers.iter().filter(|server| server.enabled) {
        let (transport, tools) = discover_mcp_server_tools(server, &workspace)?;
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
        workspace,
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
    workspace: &Path,
) -> Result<(McpTransport, Vec<Value>), String> {
    let mut errors = Vec::new();
    for transport in transport_candidates(server.transport) {
        match mcp_json_rpc(server, transport, "tools/list", json!({}), workspace) {
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
    workspace: &Path,
) -> Result<Value, String> {
    if transport == McpTransport::Stdio {
        return stdio_mcp_json_rpc(server, method, params, workspace);
    }
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
        return Err(format!("MCP JSON-RPC error: {}", stable_json_dumps(error)));
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
        &runtime.workspace,
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

struct StdioMcpSession {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Result<Value, String>>,
    timeout: Duration,
}

impl StdioMcpSession {
    fn spawn(server: &RemoteMcpServerConfig, workspace: &Path) -> Result<Self, String> {
        let Some((program, args)) = server.command.split_first() else {
            return Err(format!(
                "MCP server '{}' is missing a command.",
                server.name
            ));
        };
        let mut command = ProcessCommand::new(program);
        command.args(args);
        command.current_dir(resolve_stdio_cwd(server, workspace));
        command.envs(&server.environment);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start MCP server '{}': {error}", server.name))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("MCP server '{}' stdout was not captured", server.name))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("MCP server '{}' stdin was not captured", server.name))?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_stdio_message(&mut reader) {
                    Ok(value) => {
                        if tx.send(Ok(value)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            rx,
            timeout: Duration::from_millis(server.timeout_ms),
        })
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = format!("openagent-{}", now_ms_cli());
        self.write(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        self.read_response(&id)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn write(&mut self, message: Value) -> Result<(), String> {
        let raw = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        self.stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", raw.len()).as_bytes())
            .map_err(|error| format!("failed to write MCP frame header: {error}"))?;
        self.stdin
            .write_all(&raw)
            .map_err(|error| format!("failed to write MCP frame body: {error}"))?;
        self.stdin
            .flush()
            .map_err(|error| format!("failed to flush MCP frame: {error}"))
    }

    fn read_response(&mut self, id: &str) -> Result<Value, String> {
        loop {
            let value = match self.rx.recv_timeout(self.timeout) {
                Ok(Ok(value)) => value,
                Ok(Err(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "MCP stdio request timed out after {:?}",
                        self.timeout
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("MCP stdio server closed stdout".to_string());
                }
            };
            if value.get("id").and_then(Value::as_str) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(format!("MCP JSON-RPC error: {}", stable_json_dumps(error)));
            }
            return Ok(value.get("result").cloned().unwrap_or(value));
        }
    }

    fn close(mut self) {
        let _ = self.write(json!({
            "jsonrpc": "2.0",
            "id": format!("openagent-shutdown-{}", now_ms_cli()),
            "method": "shutdown",
            "params": {},
        }));
        let _ = self.notify("exit", json!({}));
        for _ in 0..10 {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn stdio_mcp_json_rpc(
    server: &RemoteMcpServerConfig,
    method: &str,
    params: Value,
    workspace: &Path,
) -> Result<Value, String> {
    let mut session = StdioMcpSession::spawn(server, workspace)?;
    session.request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"roots": {}},
            "clientInfo": {"name": "openagent", "version": env!("CARGO_PKG_VERSION")},
        }),
    )?;
    session.notify("notifications/initialized", json!({}))?;
    let result = session.request(method, params);
    session.close();
    result
}

fn resolve_stdio_cwd(server: &RemoteMcpServerConfig, workspace: &Path) -> PathBuf {
    let Some(cwd) = server
        .cwd
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return workspace.to_path_buf();
    };
    let path = PathBuf::from(cwd);
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}

fn read_stdio_message<R: BufRead>(reader: &mut R) -> Result<Value, String> {
    let mut content_length = None::<usize>;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| format!("failed to read MCP frame header: {error}"))?;
        if read == 0 {
            return Err("MCP stdio server closed before sending a frame".to_string());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| format!("invalid MCP content-length: {error}"))?,
            );
        }
    }
    let length = content_length.ok_or_else(|| "MCP frame missing content-length".to_string())?;
    let mut body = vec![0_u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("failed to read MCP frame body: {error}"))?;
    serde_json::from_slice::<Value>(&body)
        .map_err(|error| format!("MCP frame body was not JSON: {error}"))
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
