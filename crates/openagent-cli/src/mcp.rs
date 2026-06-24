use super::prompt::discover_mcp_server_tools;
use super::*;

pub(super) fn mcp_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(mcp_help());
    }
    match args[0].as_str() {
        "add" => mcp_add(&args[1..]),
        "list" | "ls" => mcp_list(&args[1..]),
        "show" => mcp_show(&args[1..]),
        "remove" | "rm" => mcp_remove(&args[1..]),
        "auth" => mcp_auth(&args[1..]),
        "logout" => mcp_logout(&args[1..]),
        "doctor" => mcp_doctor(&args[1..]),
        "debug" => mcp_debug(&args[1..]),
        _ => err_text(2, format!("unknown mcp command: {}", args[0])),
    }
}

fn mcp_add(args: &[String]) -> CliRunResult {
    let positionals = positional_args(
        args,
        &[
            "--config",
            "--workspace",
            "--dir",
            "--url",
            "--transport",
            "--header",
            "--timeout-ms",
            "--format",
        ],
    );
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp add requires a server name");
    };
    let Some(url) = value_for(args, &["--url"]) else {
        return err_text(2, "mcp add requires --url");
    };
    let config_path = mcp_config_path(args);
    let mut config = read_json_file(&config_path);
    let servers = ensure_object_field(&mut config, "mcp");
    let headers = parse_headers(&values_for(args, &["--header"]));
    let server = json!({
        "type": "remote",
        "url": url,
        "transport": value_for(args, &["--transport"]).unwrap_or_else(|| "auto".to_string()),
        "enabled": !has_flag(args, &["--disabled"]),
        "timeout_ms": value_for(args, &["--timeout-ms"]).and_then(|value| value.parse::<u64>().ok()).unwrap_or(30_000),
        "headers": headers,
    });
    servers.insert(name.clone(), server);
    let public_server = mcp_public_server(name, servers.get(name).unwrap_or(&Value::Null));
    if let Err(error) = write_json_file(&config_path, &config) {
        return err_text(1, error);
    }
    let payload = json!({"config_path": config_path.to_string_lossy(), "server": public_server, "updated": true});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!("updated MCP server {name}"))
    }
}

fn mcp_list(args: &[String]) -> CliRunResult {
    let config_path = mcp_config_path(args);
    let servers = mcp_public_servers(&read_json_file(&config_path));
    let payload = json!({"config_path": config_path.to_string_lossy(), "servers": servers});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else if payload["servers"].as_array().is_none_or(Vec::is_empty) {
        ok_text("No MCP servers configured")
    } else {
        let rows = payload["servers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|server| {
                let headers = server["header_names"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",");
                vec![
                    server["name"].as_str().unwrap_or("").to_string(),
                    if server["enabled"].as_bool().unwrap_or(false) {
                        "yes".to_string()
                    } else {
                        "no".to_string()
                    },
                    server["transport"].as_str().unwrap_or("auto").to_string(),
                    server["timeout_ms"]
                        .as_u64()
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    if headers.is_empty() {
                        "-".to_string()
                    } else {
                        headers
                    },
                    server["url"].as_str().unwrap_or("").to_string(),
                ]
            })
            .collect::<Vec<_>>();
        ok_text(format!(
            "MCP Servers\n{}",
            render_table(
                &["Name", "Enabled", "Transport", "Timeout", "Headers", "URL"],
                &rows
            )
        ))
    }
}

fn mcp_show(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--config", "--workspace", "--dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp show requires a server name");
    };
    let config_path = mcp_config_path(args);
    let config = read_json_file(&config_path);
    let server = config
        .get("mcp")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(name));
    let Some(server) = server else {
        return err_text(1, format!("MCP server not found: {name}"));
    };
    let payload = json!({"config_path": config_path.to_string_lossy(), "server": mcp_public_server(name, server)});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!(
            "{} {}",
            name,
            payload["server"]["url"].as_str().unwrap_or("")
        ))
    }
}

fn mcp_remove(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--config", "--workspace", "--dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp remove requires a server name");
    };
    let config_path = mcp_config_path(args);
    let mut config = read_json_file(&config_path);
    let removed = config
        .get_mut("mcp")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.remove(name))
        .is_some();
    if let Err(error) = write_json_file(&config_path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"config_path": config_path.to_string_lossy(), "name": name, "removed": removed}),
    )
}

fn mcp_auth(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(
            "Usage: openagent mcp auth <list|status|login|set-token|callback> [options]",
        );
    }
    match args[0].as_str() {
        "list" | "ls" | "status" => mcp_doctor(&args[1..]),
        "login" | "start" => mcp_auth_login(&args[1..]),
        "callback" => mcp_auth_callback(&args[1..]),
        "set-token" => {
            let positionals = positional_args(
                &args[1..],
                &[
                    "--config",
                    "--workspace",
                    "--dir",
                    "--bearer-token",
                    "--header-name",
                    "--format",
                ],
            );
            let Some(name) = positionals.first() else {
                return err_text(2, "mcp auth set-token requires a server name");
            };
            let Some(token) = value_for(&args[1..], &["--bearer-token"]) else {
                return err_text(
                    2,
                    "mcp auth set-token requires --bearer-token in this Rust CLI path",
                );
            };
            let header = value_for(&args[1..], &["--header-name"])
                .unwrap_or_else(|| "Authorization".to_string());
            let config_path = mcp_config_path(&args[1..]);
            let mut config = read_json_file(&config_path);
            let Some(server) = config
                .get_mut("mcp")
                .and_then(Value::as_object_mut)
                .and_then(|servers| servers.get_mut(name))
                .and_then(Value::as_object_mut)
            else {
                return err_text(1, format!("MCP server not found: {name}"));
            };
            let headers = server.entry("headers").or_insert_with(|| json!({}));
            if let Some(headers) = headers.as_object_mut() {
                headers.insert(header.clone(), json!(format!("Bearer {token}")));
            }
            if let Err(error) = write_json_file(&config_path, &config) {
                return err_text(1, error);
            }
            CliRunResult::ok_json(
                &json!({"config_path": config_path.to_string_lossy(), "name": name, "header": header, "updated": true}),
            )
        }
        _ => err_text(2, format!("unknown mcp auth command: {}", args[0])),
    }
}

fn mcp_auth_login(args: &[String]) -> CliRunResult {
    let positionals = positional_args(
        args,
        &[
            "--config",
            "--workspace",
            "--dir",
            "--client-id",
            "--client-secret",
            "--authorize-url",
            "--token-url",
            "--redirect-uri",
            "--scope",
            "--format",
        ],
    );
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp auth login requires a server name");
    };
    let config_path = mcp_config_path(args);
    let mut config = read_json_file(&config_path);
    let Some(server) = config
        .get_mut("mcp")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.get_mut(name))
        .and_then(Value::as_object_mut)
    else {
        return err_text(1, format!("MCP server not found: {name}"));
    };
    let state = new_cli_id("mcp_oauth");
    let redirect_uri = value_for(args, &["--redirect-uri"])
        .unwrap_or_else(|| "http://127.0.0.1:8787/mcp/oauth/callback".to_string());
    let authorize_url = value_for(args, &["--authorize-url"])
        .or_else(|| {
            server
                .get("authorize_url")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            let url = server
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("{}/authorize", url.trim_end_matches('/'))
        });
    let client_id =
        value_for(args, &["--client-id"]).unwrap_or_else(|| "openagent-cli".to_string());
    let scope = value_for(args, &["--scope"]).unwrap_or_else(|| "mcp".to_string());
    let url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
        authorize_url,
        url_encode(&client_id),
        url_encode(&redirect_uri),
        url_encode(&scope),
        url_encode(&state)
    );
    server.insert(
        "oauth".to_string(),
        json!({
            "state": state,
            "client_id": client_id,
            "client_secret": value_for(args, &["--client-secret"]).unwrap_or_default(),
            "authorize_url": authorize_url,
            "token_url": value_for(args, &["--token-url"]),
            "redirect_uri": redirect_uri,
            "scope": scope,
            "status": "authorization_required",
            "updated_at_ms": now_ms_cli(),
        }),
    );
    if let Err(error) = write_json_file(&config_path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(&json!({
        "config_path": config_path.to_string_lossy(),
        "name": name,
        "status": "authorization_required",
        "authorize_url": url,
    }))
}

fn mcp_auth_callback(args: &[String]) -> CliRunResult {
    let positionals = positional_args(
        args,
        &[
            "--config",
            "--workspace",
            "--dir",
            "--code",
            "--state",
            "--access-token",
            "--format",
        ],
    );
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp auth callback requires a server name");
    };
    let config_path = mcp_config_path(args);
    let mut config = read_json_file(&config_path);
    let Some(server) = config
        .get_mut("mcp")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.get_mut(name))
        .and_then(Value::as_object_mut)
    else {
        return err_text(1, format!("MCP server not found: {name}"));
    };
    let expected_state = server
        .get("oauth")
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(state) = value_for(args, &["--state"])
        && !expected_state.is_empty()
        && state != expected_state
    {
        return err_text(1, "MCP OAuth state mismatch");
    }
    let access_token = value_for(args, &["--access-token"]).unwrap_or_else(|| {
        value_for(args, &["--code"])
            .map(|code| format!("code:{code}"))
            .unwrap_or_default()
    });
    if access_token.is_empty() {
        return err_text(2, "mcp auth callback requires --code or --access-token");
    }
    let headers = server.entry("headers").or_insert_with(|| json!({}));
    if let Some(headers) = headers.as_object_mut() {
        headers.insert(
            "Authorization".to_string(),
            json!(format!("Bearer {access_token}")),
        );
    }
    server.insert(
        "oauth".to_string(),
        json!({"status": "authorized", "updated_at_ms": now_ms_cli()}),
    );
    if let Err(error) = write_json_file(&config_path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"config_path": config_path.to_string_lossy(), "name": name, "status": "authorized"}),
    )
}

fn mcp_logout(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--config", "--workspace", "--dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp logout requires a server name");
    };
    let config_path = mcp_config_path(args);
    let mut config = read_json_file(&config_path);
    let removed = config
        .get_mut("mcp")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.get_mut(name))
        .and_then(Value::as_object_mut)
        .and_then(|server| server.get_mut("headers"))
        .and_then(Value::as_object_mut)
        .and_then(|headers| headers.remove("Authorization"))
        .is_some();
    if let Err(error) = write_json_file(&config_path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"config_path": config_path.to_string_lossy(), "name": name, "removed": removed}),
    )
}

fn mcp_doctor(args: &[String]) -> CliRunResult {
    let config_path = mcp_config_path(args);
    let config = if config_path.exists() {
        match load_mcp_config(&config_path.to_string_lossy()) {
            Ok(config) => config,
            Err(error) => return err_text(1, error),
        }
    } else {
        openagent_mcp::McpConfig::default()
    };
    let refresh = has_flag(args, &["--refresh"]);
    let mut manager = RemoteMcpManager::new(config.clone());
    let mut refresh_error = None::<String>;
    if refresh {
        for server in config.servers.iter().filter(|server| server.enabled) {
            match discover_mcp_server_tools(server) {
                Ok((transport, tools)) => {
                    let descriptors = build_tool_descriptors_from_values(server, &tools);
                    let _ = manager.set_server_tools(
                        &server.name,
                        Some(transport),
                        "connected",
                        Some(now_ms_cli() as f64 / 1000.0),
                        descriptors,
                    );
                }
                Err(error) => {
                    refresh_error = Some(error);
                }
            }
        }
    }
    let snapshot = serde_json::to_value(manager.snapshot()).unwrap_or_else(|_| json!({}));
    let servers = snapshot
        .get("servers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let payload = json!({
        "config_path": config_path.to_string_lossy(),
        "configured": !servers.is_empty(),
        "enabled": servers.iter().any(|server| server["enabled"].as_bool().unwrap_or(false)),
        "server_count": servers.len(),
        "ok": refresh_error.is_none() && servers.iter().all(|server| server["status"].as_str() != Some("failed")),
        "refresh_error": refresh_error,
        "servers": servers,
    });
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!(
            "{} MCP server(s)",
            payload["server_count"].as_u64().unwrap_or(0)
        ))
    }
}

fn mcp_debug(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--config", "--workspace", "--dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp debug requires a server name");
    };
    let config_path = mcp_config_path(args);
    let config = read_json_file(&config_path);
    let server = config
        .get("mcp")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(name));
    let Some(server) = server else {
        return err_text(1, format!("MCP server not found: {name}"));
    };
    CliRunResult::ok_json(&json!({"server": mcp_public_server(name, server)}))
}
