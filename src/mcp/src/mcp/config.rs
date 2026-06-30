#[must_use]
pub const fn is_default_server_type(value: &McpServerType) -> bool {
    matches!(value, McpServerType::Remote)
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
    let (mcp_block, default_timeout_ms) = mcp_servers_block(raw_object, raw);
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
        servers.push(parse_server_config(
            trimmed,
            server_object,
            default_timeout_ms,
        )?);
    }
    Ok(McpConfig {
        servers,
        refresh_ttl_s,
    })
}

fn mcp_servers_block<'a>(raw_object: &'a Map<String, Value>, raw: &'a Value) -> (&'a Value, u64) {
    if let Some(mcp_servers) = raw_object.get("mcpServers") {
        return (mcp_servers, DEFAULT_TIMEOUT_MS);
    }
    if let Some(mcp) = raw_object.get("mcp") {
        if let Some(mcp_object) = mcp.as_object()
            && let Some(servers) = mcp_object.get("servers")
        {
            let default_timeout = parse_int(
                mcp_object
                    .get("timeout_ms")
                    .or_else(|| mcp_object.get("timeout")),
                DEFAULT_TIMEOUT_MS,
                MIN_TIMEOUT_MS,
            );
            return (servers, default_timeout);
        }
        return (mcp, DEFAULT_TIMEOUT_MS);
    }
    (raw, DEFAULT_TIMEOUT_MS)
}

fn parse_server_config(
    name: &str,
    raw: &Map<String, Value>,
    default_timeout_ms: u64,
) -> McpResult<RemoteMcpServerConfig> {
    let has_command = raw.get("command").is_some_and(legacy_truthy);
    let type_value = raw
        .get("type")
        .map_or_else(
            || {
                if has_command {
                    "local".to_string()
                } else {
                    "remote".to_string()
                }
            },
            value_to_legacy_string,
        )
        .trim()
        .to_ascii_lowercase();
    let (server_type, default_transport) = match type_value.as_str() {
        "local" | "stdio" => (McpServerType::Local, McpTransport::Stdio),
        "streamablehttp" | "streamable_http" | "http" => {
            (McpServerType::Remote, McpTransport::Http)
        }
        "sse" => (McpServerType::Remote, McpTransport::Sse),
        "remote" => (McpServerType::Remote, McpTransport::Auto),
        other => {
            return Err(format!(
                "MCP server '{name}' has unsupported type '{other}'. Supported values are remote, local, streamableHttp, sse, and stdio."
            ));
        }
    };

    let url = raw
        .get("url")
        .filter(|value| legacy_truthy(value))
        .map_or_else(String::new, value_to_legacy_string)
        .trim()
        .to_string();
    if server_type == McpServerType::Remote && url.is_empty() {
        return Err(format!("MCP server '{name}' is missing a non-empty url."));
    }
    let command = parse_local_command(name, raw, server_type)?;
    if server_type == McpServerType::Local && command.is_empty() {
        return Err(format!(
            "MCP server '{name}' is missing a non-empty command."
        ));
    }

    let transport_raw = raw.get("transport").filter(|value| legacy_truthy(value));
    let transport_text = transport_raw
        .map_or_else(
            || default_transport.as_str().to_string(),
            value_to_legacy_string,
        )
        .trim()
        .to_ascii_lowercase();
    let transport = match transport_text.as_str() {
        "auto" => McpTransport::Auto,
        "http" => McpTransport::Http,
        "sse" => McpTransport::Sse,
        "stdio" => McpTransport::Stdio,
        other => {
            return Err(format!(
                "MCP server '{name}' has unsupported transport '{other}'. Supported values are auto, http, sse, stdio."
            ));
        }
    };
    if server_type == McpServerType::Local
        && !matches!(transport, McpTransport::Auto | McpTransport::Stdio)
    {
        return Err(format!(
            "MCP server '{name}' is local and must use transport='stdio' or 'auto'."
        ));
    }

    Ok(RemoteMcpServerConfig {
        name: name.to_string(),
        server_type,
        url,
        transport: if server_type == McpServerType::Local {
            McpTransport::Stdio
        } else {
            transport
        },
        enabled: parse_enabled(raw),
        command,
        cwd: raw
            .get("cwd")
            .filter(|value| legacy_truthy(value))
            .map(value_to_legacy_string),
        environment: normalize_environment(raw.get("environment").or_else(|| raw.get("env")))?,
        headers: normalize_headers(raw.get("headers"))?,
        timeout_ms: parse_int(
            raw.get("timeout_ms").or_else(|| raw.get("timeout")),
            default_timeout_ms,
            MIN_TIMEOUT_MS,
        ),
        tools: parse_tool_filter(raw.get("tools"))?,
    })
}

fn parse_enabled(raw: &Map<String, Value>) -> bool {
    let enabled = raw.get("enabled").is_none_or(legacy_truthy);
    let disabled = raw.get("disabled").is_some_and(legacy_truthy);
    enabled && !disabled
}

fn parse_local_command(
    name: &str,
    raw: &Map<String, Value>,
    server_type: McpServerType,
) -> McpResult<Vec<String>> {
    if server_type != McpServerType::Local {
        return Ok(Vec::new());
    }
    let mut command = match raw.get("command") {
        Some(Value::Array(items)) => items
            .iter()
            .map(value_to_legacy_string)
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>(),
        Some(value) if legacy_truthy(value) => {
            let text = value_to_legacy_string(value).trim().to_string();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![text]
            }
        }
        _ => Vec::new(),
    };
    if let Some(args) = raw.get("args") {
        let Some(items) = args.as_array() else {
            return Err(format!("MCP server '{name}' args must be a string array."));
        };
        command.extend(
            items
                .iter()
                .map(value_to_legacy_string)
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty()),
        );
    }
    Ok(command)
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
        .map(value_to_legacy_string)
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
        headers.insert(header.to_string(), value_to_legacy_string(value));
    }
    Ok(headers)
}

fn normalize_environment(raw: Option<&Value>) -> McpResult<BTreeMap<String, String>> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    let Some(object) = raw.as_object() else {
        return Err("MCP environment must be an object.".to_string());
    };
    let mut environment = BTreeMap::new();
    for (key, value) in object {
        let name = key.trim();
        if name.is_empty() {
            continue;
        }
        environment.insert(name.to_string(), value_to_legacy_string(value));
    }
    Ok(environment)
}
