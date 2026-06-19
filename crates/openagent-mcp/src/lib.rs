//! MCP config, auth, discovery, and tool bridge crate for the Rust rewrite.

use std::{collections::BTreeMap, fs, path::Path};

use openagent_protocol::{ToolConcurrency, ToolExecutionSchema, ToolExecutionScope};
use openagent_tools::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const DEFAULT_REFRESH_TTL_S: f64 = 30.0;
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MIN_TIMEOUT_MS: u64 = 1_000;

const SENSITIVE_KEY_MARKERS: &[&str] = &[
    "api_key",
    "apikey",
    "authorization",
    "cookie",
    "password",
    "secret",
    "token",
];
const SAFE_TOKEN_METRIC_KEYS: &[&str] = &[
    "estimated_input_tokens",
    "input_limit_tokens",
    "input_tokens",
    "max_output_tokens",
    "output_tokens",
    "reserved_output_tokens",
];

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn tool_crate_name() -> &'static str {
    openagent_tools::crate_name()
}

pub type McpResult<T> = Result<T, String>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Auto,
    Http,
    Sse,
}

impl McpTransport {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpToolFilter {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

impl Default for McpToolFilter {
    fn default() -> Self {
        Self {
            allow: vec!["*".to_string()],
            deny: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteMcpServerConfig {
    pub name: String,
    pub url: String,
    pub transport: McpTransport,
    pub enabled: bool,
    pub headers: BTreeMap<String, String>,
    pub timeout_ms: u64,
    pub tools: McpToolFilter,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct McpConfig {
    pub servers: Vec<RemoteMcpServerConfig>,
    pub refresh_ttl_s: f64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            refresh_ttl_s: DEFAULT_REFRESH_TTL_S,
        }
    }
}

impl McpConfig {
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.servers.iter().any(|server| server.enabled)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteMcpToolDescriptor {
    pub server_name: String,
    pub original_name: String,
    pub dynamic_name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Value,
    pub raw_metadata: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteMcpToolCallResult {
    pub output: String,
    pub error: Option<String>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteMcpServerSnapshot {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub configured_transport: McpTransport,
    pub selected_transport: Option<McpTransport>,
    pub status: String,
    pub tool_count: usize,
    pub last_error: Option<String>,
    pub last_refreshed_at: Option<f64>,
    pub tools: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteMcpSnapshot {
    pub configured: bool,
    pub enabled: bool,
    pub server_count: usize,
    pub servers: Vec<RemoteMcpServerSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteMcpServerState {
    pub config: RemoteMcpServerConfig,
    pub status: String,
    pub selected_transport: Option<McpTransport>,
    pub last_error: Option<String>,
    pub last_refreshed_at: Option<f64>,
    pub tools_by_dynamic_name: BTreeMap<String, RemoteMcpToolDescriptor>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteMcpManager {
    pub config: McpConfig,
    pub servers: BTreeMap<String, RemoteMcpServerState>,
}

impl RemoteMcpManager {
    #[must_use]
    pub fn new(config: McpConfig) -> Self {
        let servers = config
            .servers
            .iter()
            .map(|server| {
                (
                    server.name.clone(),
                    RemoteMcpServerState {
                        config: server.clone(),
                        status: if server.enabled { "idle" } else { "disabled" }.to_string(),
                        selected_transport: None,
                        last_error: None,
                        last_refreshed_at: None,
                        tools_by_dynamic_name: BTreeMap::new(),
                    },
                )
            })
            .collect();
        Self { config, servers }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.config.enabled()
    }

    #[must_use]
    pub fn snapshot(&self) -> RemoteMcpSnapshot {
        let servers = self
            .servers
            .values()
            .map(|state| {
                let tools = state
                    .tools_by_dynamic_name
                    .values()
                    .map(|descriptor| {
                        json!({
                            "name": descriptor.dynamic_name,
                            "original_name": descriptor.original_name,
                            "title": descriptor.title,
                            "description": descriptor.description,
                        })
                    })
                    .collect::<Vec<_>>();
                RemoteMcpServerSnapshot {
                    name: state.config.name.clone(),
                    url: state.config.url.clone(),
                    enabled: state.config.enabled,
                    configured_transport: state.config.transport,
                    selected_transport: state.selected_transport,
                    status: state.status.clone(),
                    tool_count: state.tools_by_dynamic_name.len(),
                    last_error: state.last_error.clone(),
                    last_refreshed_at: state.last_refreshed_at,
                    tools,
                }
            })
            .collect::<Vec<_>>();
        RemoteMcpSnapshot {
            configured: !self.servers.is_empty(),
            enabled: self.enabled(),
            server_count: servers.len(),
            servers,
        }
    }

    #[must_use]
    pub fn list_tool_descriptors(&self) -> Vec<RemoteMcpToolDescriptor> {
        let mut tools = self
            .servers
            .values()
            .flat_map(|state| state.tools_by_dynamic_name.values().cloned())
            .collect::<Vec<_>>();
        tools.sort_by(|left, right| left.dynamic_name.cmp(&right.dynamic_name));
        tools
    }

    pub fn set_server_tools(
        &mut self,
        server_name: &str,
        selected_transport: Option<McpTransport>,
        status: impl Into<String>,
        last_refreshed_at: Option<f64>,
        descriptors: Vec<RemoteMcpToolDescriptor>,
    ) -> McpResult<()> {
        let Some(state) = self.servers.get_mut(server_name) else {
            return Err(format!("Unknown MCP server: {server_name}"));
        };
        state.selected_transport = selected_transport;
        state.status = status.into();
        state.last_error = None;
        state.last_refreshed_at = last_refreshed_at;
        state.tools_by_dynamic_name = descriptors
            .into_iter()
            .map(|descriptor| (descriptor.dynamic_name.clone(), descriptor))
            .collect();
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct McpBridgeOutput {
    pub title: String,
    pub output: String,
    pub metadata: BTreeMap<String, Value>,
    pub truncated: bool,
    pub attachments: Vec<Value>,
    pub error: Option<String>,
}

pub fn load_mcp_config_from_sources(
    cli_value: Option<&str>,
    env: &BTreeMap<String, String>,
) -> McpResult<Option<McpConfig>> {
    let source = cli_value.or_else(|| env.get("OPENAGENT_MCP_CONFIG").map(String::as_str));
    let Some(source) = source else {
        return Ok(None);
    };
    if source.trim().is_empty() {
        return Ok(None);
    }
    load_mcp_config(source).map(Some)
}

pub fn load_mcp_config(source: &str) -> McpResult<McpConfig> {
    let candidate = Path::new(source);
    if candidate.exists() {
        let raw = fs::read_to_string(candidate)
            .map_err(|error| format!("MCP config file not found: {candidate:?}: {error}"))?;
        let value = serde_json::from_str::<Value>(&raw)
            .map_err(|_| format!("MCP config file is not valid JSON: {}", candidate.display()))?;
        return load_mcp_config_from_value(&value);
    }
    let value = serde_json::from_str::<Value>(source).map_err(|_| {
        "MCP config must be a valid JSON string or a path to a JSON file.".to_string()
    })?;
    if !value.is_object() {
        return Err("MCP config JSON must be an object.".to_string());
    }
    load_mcp_config_from_value(&value)
}

pub fn load_mcp_config_from_value(raw: &Value) -> McpResult<McpConfig> {
    let Some(raw_object) = raw.as_object() else {
        return Err("MCP config must be a JSON object.".to_string());
    };
    let mcp_block = raw_object
        .get("mcpServers")
        .or_else(|| raw_object.get("mcp"))
        .unwrap_or(raw);
    let Some(servers_object) = mcp_block.as_object() else {
        return Err(
            "MCP config must contain an object-valued 'mcp' or 'mcpServers' field.".to_string(),
        );
    };
    let refresh_ttl_s = parse_float(raw_object.get("refresh_ttl_s"), DEFAULT_REFRESH_TTL_S, 0.0);
    let mut servers = Vec::new();
    for (server_name, server_raw) in servers_object {
        let trimmed = server_name.trim();
        if trimmed.is_empty() {
            return Err("MCP server names must be non-empty strings.".to_string());
        }
        let Some(server_object) = server_raw.as_object() else {
            return Err(format!(
                "MCP server '{trimmed}' must be configured with an object."
            ));
        };
        servers.push(parse_server_config(trimmed, server_object)?);
    }
    Ok(McpConfig {
        servers,
        refresh_ttl_s,
    })
}

fn parse_server_config(name: &str, raw: &Map<String, Value>) -> McpResult<RemoteMcpServerConfig> {
    let type_value = raw
        .get("type")
        .map_or_else(|| "remote".to_string(), value_to_python_string)
        .trim()
        .to_ascii_lowercase();
    let default_transport = if matches!(
        type_value.as_str(),
        "streamablehttp" | "streamable_http" | "http"
    ) {
        McpTransport::Http
    } else if type_value == "sse" {
        McpTransport::Sse
    } else if type_value == "remote" {
        McpTransport::Auto
    } else {
        return Err(format!(
            "MCP server '{name}' only supports type='remote', 'streamableHttp', or 'sse' in v1."
        ));
    };

    let url = raw
        .get("url")
        .filter(|value| python_truthy(value))
        .map_or_else(String::new, value_to_python_string)
        .trim()
        .to_string();
    if url.is_empty() {
        return Err(format!("MCP server '{name}' is missing a non-empty url."));
    }

    let transport_raw = raw.get("transport").filter(|value| python_truthy(value));
    let transport_text = transport_raw
        .map_or_else(
            || default_transport.as_str().to_string(),
            value_to_python_string,
        )
        .trim()
        .to_ascii_lowercase();
    let transport = match transport_text.as_str() {
        "auto" => McpTransport::Auto,
        "http" => McpTransport::Http,
        "sse" => McpTransport::Sse,
        other => {
            return Err(format!(
                "MCP server '{name}' has unsupported transport '{other}'. Supported values are auto, http, sse."
            ));
        }
    };

    Ok(RemoteMcpServerConfig {
        name: name.to_string(),
        url,
        transport,
        enabled: raw.get("enabled").is_none_or(python_truthy),
        headers: normalize_headers(raw.get("headers"))?,
        timeout_ms: parse_int(raw.get("timeout_ms"), DEFAULT_TIMEOUT_MS, MIN_TIMEOUT_MS),
        tools: parse_tool_filter(raw.get("tools"))?,
    })
}

fn parse_tool_filter(raw: Option<&Value>) -> McpResult<McpToolFilter> {
    let Some(raw) = raw else {
        return Ok(McpToolFilter::default());
    };
    let Some(object) = raw.as_object() else {
        return Err("MCP tools filter must be an object with allow/deny arrays.".to_string());
    };
    Ok(McpToolFilter {
        allow: normalize_pattern_list(object.get("allow"), &["*"])?,
        deny: normalize_pattern_list(object.get("deny"), &[])?,
    })
}

fn normalize_pattern_list(raw: Option<&Value>, default: &[&str]) -> McpResult<Vec<String>> {
    let Some(raw) = raw else {
        return Ok(default.iter().map(|value| (*value).to_string()).collect());
    };
    let Some(items) = raw.as_array() else {
        return Err("MCP tool filters must use string arrays.".to_string());
    };
    let values = items
        .iter()
        .map(value_to_python_string)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        Ok(default.iter().map(|value| (*value).to_string()).collect())
    } else {
        Ok(values)
    }
}

fn normalize_headers(raw: Option<&Value>) -> McpResult<BTreeMap<String, String>> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    let Some(object) = raw.as_object() else {
        return Err("MCP headers must be an object.".to_string());
    };
    let mut headers = BTreeMap::new();
    for (key, value) in object {
        let header = key.trim();
        if header.is_empty() {
            continue;
        }
        headers.insert(header.to_string(), value_to_python_string(value));
    }
    Ok(headers)
}

#[must_use]
pub fn build_tool_descriptors_from_values(
    server: &RemoteMcpServerConfig,
    tools: &[Value],
) -> Vec<RemoteMcpToolDescriptor> {
    let mut descriptors = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for tool in tools {
        let original_name = tool
            .get("name")
            .filter(|value| python_truthy(value))
            .map_or_else(String::new, value_to_python_string)
            .trim()
            .to_string();
        if original_name.is_empty() || !tool_allowed(&original_name, &server.tools) {
            continue;
        }

        let mut dynamic_name = dynamic_tool_name(&server.name, &original_name);
        if seen.contains(&dynamic_name) {
            dynamic_name = format!(
                "{}_{}",
                dynamic_name,
                duplicate_suffix(&server.name, &original_name)
            );
        }
        seen.insert(dynamic_name.clone());

        let title = tool
            .get("title")
            .filter(|value| python_truthy(value))
            .map_or_else(|| original_name.clone(), value_to_python_string);
        let raw_description = tool
            .get("description")
            .filter(|value| python_truthy(value))
            .map_or_else(String::new, value_to_python_string);
        let description = tool_description(&server.name, &original_name, &raw_description);
        let input_schema = normalize_input_schema(tool.get("inputSchema"));
        let annotations_safe = tool
            .get("annotations")
            .filter(|value| python_truthy(value))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let annotations = if annotations_safe.is_object() {
            annotations_safe.clone()
        } else {
            json!({})
        };
        let execution = tool.get("execution").cloned().unwrap_or(Value::Null);
        let raw_metadata = json!({
            "title": title,
            "description": raw_description,
            "annotations": annotations_safe,
            "execution": execution,
        });

        descriptors.push(RemoteMcpToolDescriptor {
            server_name: server.name.clone(),
            original_name,
            dynamic_name,
            title,
            description,
            input_schema,
            annotations,
            raw_metadata,
        });
    }

    descriptors
}

#[must_use]
pub fn tool_allowed(tool_name: &str, filters: &McpToolFilter) -> bool {
    if !filters.allow.is_empty()
        && !filters
            .allow
            .iter()
            .any(|pattern| fnmatchcase(tool_name, pattern))
    {
        return false;
    }
    if filters
        .deny
        .iter()
        .any(|pattern| fnmatchcase(tool_name, pattern))
    {
        return false;
    }
    true
}

#[must_use]
pub fn transport_candidates(transport: McpTransport) -> Vec<McpTransport> {
    match transport {
        McpTransport::Http => vec![McpTransport::Http],
        McpTransport::Sse => vec![McpTransport::Sse],
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

#[must_use]
pub fn normalize_tool_call_result(
    descriptor: &RemoteMcpToolDescriptor,
    transport: Option<McpTransport>,
    result: &Value,
) -> RemoteMcpToolCallResult {
    let (mut output, non_text_blocks) = render_tool_result_output(result);
    let is_error = result.get("isError").is_some_and(python_truthy);
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
            .filter(|value| python_truthy(value))
            .map_or_else(String::new, value_to_python_string)
            .trim()
            .to_ascii_lowercase();
        if item_type == "text" {
            let text = item
                .get("text")
                .filter(|value| python_truthy(value))
                .map_or_else(String::new, value_to_python_string)
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

#[must_use]
pub fn sanitize_mcp_value(value: &Value, max_chars: usize) -> Value {
    match value {
        Value::Object(object) => {
            let mut sanitized = Map::new();
            for (key, item) in object {
                if is_sensitive_key(key) && !SAFE_TOKEN_METRIC_KEYS.contains(&key.as_str()) {
                    sanitized.insert(key.clone(), Value::String("[redacted]".to_string()));
                } else {
                    sanitized.insert(key.clone(), sanitize_mcp_value(item, max_chars));
                }
            }
            Value::Object(sanitized)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| sanitize_mcp_value(item, max_chars))
                .collect(),
        ),
        Value::String(text) => Value::String(truncate_text(text, max_chars)),
        _ => value.clone(),
    }
}

#[must_use]
pub fn sanitize_mcp_trace_value(value: &Value) -> Value {
    sanitize_mcp_value(value, 4096)
}

#[must_use]
pub fn sanitize_mcp_observation_value(value: &Value) -> Value {
    sanitize_mcp_value(value, 4096)
}

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

fn value_to_python_string(value: &Value) -> String {
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

fn python_truthy(value: &Value) -> bool {
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
