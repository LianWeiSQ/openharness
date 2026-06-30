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
    Stdio,
}

impl McpTransport {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Http => "http",
            Self::Sse => "sse",
            Self::Stdio => "stdio",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpServerType {
    #[default]
    Remote,
    Local,
}

impl McpServerType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Local => "local",
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
    #[serde(default, skip_serializing_if = "is_default_server_type")]
    pub server_type: McpServerType,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    pub transport: McpTransport,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
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
    #[serde(default, skip_serializing_if = "is_default_server_type")]
    pub server_type: McpServerType,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
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
                    server_type: state.config.server_type,
                    url: state.config.url.clone(),
                    command: state.config.command.clone(),
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
