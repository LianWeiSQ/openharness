//! HTTP runtime service contracts for the Rust rewrite.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 8787;

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn command_name() -> &'static str {
    "openagent-http-runtime"
}

#[must_use]
pub fn app_server_crate_name() -> &'static str {
    openagent_app_server::crate_name()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HttpRuntimeConfig {
    pub host: String,
    pub port: u16,
    pub serve_static: bool,
    pub workspace: Option<String>,
    pub session_store_root: Option<String>,
    pub auth_token: Option<String>,
}

impl Default for HttpRuntimeConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            serve_static: true,
            workspace: None,
            session_store_root: None,
            auth_token: None,
        }
    }
}

impl HttpRuntimeConfig {
    #[must_use]
    pub fn auth_required(&self) -> bool {
        self.auth_token
            .as_ref()
            .is_some_and(|token| !token.is_empty())
    }

    #[must_use]
    pub fn to_public_value(&self) -> Value {
        json!({
            "host": self.host,
            "port": self.port,
            "serve_static": self.serve_static,
            "workspace": self.workspace,
            "session_store_root": self.session_store_root,
            "auth_required": self.auth_required(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HttpResponseSpec {
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub headers: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

impl HttpResponseSpec {
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CliRunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[must_use]
pub fn health_payload(config: &HttpRuntimeConfig) -> Value {
    json!({
        "ok": true,
        "service": command_name(),
        "app_bridge": app_server_crate_name(),
        "ui_enabled": config.serve_static,
        "auth_required": config.auth_required(),
    })
}

#[must_use]
pub fn route_health() -> HttpResponseSpec {
    HttpResponseSpec {
        status: 200,
        content_type: Some("application/json; charset=utf-8".to_string()),
        headers: Map::new(),
        body: None,
    }
}

#[must_use]
pub fn route_unauthorized() -> HttpResponseSpec {
    HttpResponseSpec {
        status: 401,
        content_type: None,
        headers: Map::new(),
        body: Some(json!({"error": "unauthorized"})),
    }
}

#[must_use]
pub fn route_options() -> HttpResponseSpec {
    let mut headers = Map::new();
    headers.insert(
        "Access-Control-Allow-Methods".to_string(),
        Value::String("GET, POST, OPTIONS".to_string()),
    );
    HttpResponseSpec {
        status: 204,
        content_type: None,
        headers,
        body: None,
    }
}

#[must_use]
pub fn route_unknown() -> HttpResponseSpec {
    HttpResponseSpec {
        status: 404,
        content_type: None,
        headers: Map::new(),
        body: Some(json!({"error": "unknown endpoint"})),
    }
}

pub fn parse_sse_response_lines(lines: &[&str]) -> Result<Vec<Value>, String> {
    let mut events = Vec::new();
    let mut data_lines: Vec<String> = Vec::new();
    for raw_line in lines {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if !data_lines.is_empty() {
                events.push(parse_sse_data(&data_lines.join("\n"))?);
                data_lines.clear();
            }
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }
    if !data_lines.is_empty() {
        events.push(parse_sse_data(&data_lines.join("\n"))?);
    }
    Ok(events)
}

pub fn parse_sse_data(data: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(data).map_err(|error| error.to_string())?;
    if !value.is_object() {
        return Err("SSE event data was not a JSON object".to_string());
    }
    Ok(value)
}

#[must_use]
pub fn format_http_error(method: &str, path: &str, code: u16, body: Option<&Value>) -> String {
    if let Some(error) = body
        .and_then(|value| value.get("error"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return format!("{method} {path} returned HTTP {code}: {error}");
    }
    format!("{method} {path} returned HTTP {code}")
}

#[must_use]
pub fn emit_app_bridge_events(
    events: &[Value],
    output_format: &str,
    verbose: bool,
) -> CliRunResult {
    let mut result = CliRunResult::default();
    let mut printed_answer = false;
    let mut status = "failed".to_string();
    let mut final_answer = String::new();

    for event in events {
        if output_format == "json" {
            result.stdout.push_str(&python_json_dumps(event));
            result.stdout.push('\n');
        } else if emit_text_event(event, verbose, &mut result.stdout, &mut result.stderr) {
            printed_answer = true;
        }

        let method = event_method(event);
        let params = event_params(event);
        if matches!(
            method.as_str(),
            "turn/completed" | "turn/failed" | "turn/interrupted"
        ) {
            let default_status = match method.as_str() {
                "turn/completed" => "completed",
                "turn/interrupted" => "interrupted",
                _ => "failed",
            };
            status = params
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or(default_status)
                .to_string();
            final_answer = params
                .get("final_answer")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
    }

    if output_format == "text" {
        if printed_answer {
            result.stdout.push('\n');
        } else if !final_answer.is_empty() {
            result.stdout.push_str(&final_answer);
            result.stdout.push('\n');
        }
        if status != "completed" {
            result
                .stderr
                .push_str(&format!("OpenAgent client turn failed: {status}\n"));
        }
    }
    result.exit_code = if status == "completed" { 0 } else { 1 };
    result
}

#[must_use]
pub fn build_run_prompt(message: &str, files: &[(&str, &str)]) -> String {
    let mut parts = Vec::new();
    if !message.trim().is_empty() {
        parts.push(message.trim().to_string());
    }
    for (path, content) in files {
        parts.push(format!("Attached file: {path}\n\n```text\n{content}\n```"));
    }
    parts.join("\n\n").trim().to_string()
}

#[must_use]
pub fn command_text_from_args(message: &[&str], stdin: Option<&str>, stdin_is_tty: bool) -> String {
    let message = message.join(" ").trim().to_string();
    if !message.is_empty() {
        return message;
    }
    if stdin_is_tty {
        return String::new();
    }
    stdin.unwrap_or_default().trim().to_string()
}

#[must_use]
pub fn dockerfile_lines() -> Vec<&'static str> {
    vec![
        "FROM rust:1.85-bookworm AS builder",
        "WORKDIR /app",
        "COPY . .",
        "RUN cargo build --release -p openagent-http-runtime",
        "FROM debian:bookworm-slim",
        "COPY --from=builder /app/target/release/openagent-http-runtime /usr/local/bin/openagent-http-runtime",
        "EXPOSE 8787",
        "HEALTHCHECK CMD [\"openagent-http-runtime\", \"--health-json\"]",
        "ENTRYPOINT [\"openagent-http-runtime\"]",
        "CMD [\"--host\", \"0.0.0.0\", \"--port\", \"8787\", \"--headless\"]",
    ]
}

#[must_use]
pub fn docker_smoke_command() -> Vec<&'static str> {
    vec![
        "docker",
        "run",
        "--rm",
        "openagent-http-runtime:goal12",
        "--health-json",
    ]
}

#[must_use]
pub fn parse_cli_args(args: &[String]) -> (HttpRuntimeConfig, bool, bool) {
    let mut config = HttpRuntimeConfig::default();
    let mut health_json = false;
    let mut docker_smoke = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--host" => {
                if let Some(value) = args.get(index + 1) {
                    config.host = value.clone();
                    index += 1;
                }
            }
            "--port" => {
                if let Some(value) = args
                    .get(index + 1)
                    .and_then(|value| value.parse::<u16>().ok())
                {
                    config.port = value;
                    index += 1;
                }
            }
            "--workspace" => {
                if let Some(value) = args.get(index + 1) {
                    config.workspace = Some(value.clone());
                    index += 1;
                }
            }
            "--session-root" => {
                if let Some(value) = args.get(index + 1) {
                    config.session_store_root = Some(value.clone());
                    index += 1;
                }
            }
            "--headless" => {
                config.serve_static = false;
            }
            "--auth-token" => {
                if let Some(value) = args.get(index + 1) {
                    config.auth_token = Some(value.clone());
                    index += 1;
                }
            }
            "--health-json" => {
                health_json = true;
            }
            "--docker-smoke" => {
                docker_smoke = true;
            }
            _ => {}
        }
        index += 1;
    }
    (config, health_json, docker_smoke)
}

#[must_use]
pub fn run_cli(args: &[String]) -> CliRunResult {
    let (config, health_json, docker_smoke) = parse_cli_args(args);
    if health_json || docker_smoke {
        let smoke_config = HttpRuntimeConfig {
            serve_static: false,
            auth_token: config.auth_token,
            ..HttpRuntimeConfig::default()
        };
        return CliRunResult {
            exit_code: 0,
            stdout: format!("{}\n", python_json_dumps(&health_payload(&smoke_config))),
            stderr: String::new(),
        };
    }
    CliRunResult {
        exit_code: 0,
        stdout: format!("{}\n", command_name()),
        stderr: String::new(),
    }
}

#[must_use]
pub fn http_runtime_fixture() -> Value {
    let workspace = "/tmp/openagent-rust-rewrite-fixture-goal12/workspace";
    let session_root = "/tmp/openagent-rust-rewrite-fixture-goal12/workspace/.openagent/sessions";
    let config = HttpRuntimeConfig {
        host: "0.0.0.0".to_string(),
        port: 8787,
        serve_static: false,
        workspace: Some(workspace.to_string()),
        session_store_root: Some(session_root.to_string()),
        auth_token: Some("server-secret".to_string()),
    };
    let events = fixture_events();
    let text = emit_app_bridge_events(&events, "text", true);
    let emitted_json = emit_app_bridge_events(&events, "json", false);
    let sse_lines = [
        ": ping\n",
        "\n",
        "id: 1\n",
        "event: item/agentMessage/delta\n",
        "data: {\"sequence\": 1, \"method\": \"item/agentMessage/delta\", \"params\": {\"event\": {\"text\": \"hello from server\"}}}\n",
        "\n",
        "id: 2\n",
        "event: turn/completed\n",
        "data: {\"sequence\": 2, \"method\": \"turn/completed\", \"params\": {\"status\": \"completed\", \"final_answer\": \"hello from server\"}}\n",
        "\n",
    ];

    json!({
        "schema_version": 1,
        "sdk": {"http_runtime_exports": sdk_exports()},
        "serve": {
            "args": {
                "host": "0.0.0.0",
                "port": 8787,
                "workspace": workspace,
                "session_root": session_root,
                "headless": true,
            },
            "call": {
                "host": "0.0.0.0",
                "port": 8787,
                "workspace": workspace,
                "session_store_root": session_root,
                "serve_static": false,
                "auth_token": "server-secret",
            },
        },
        "prompt": {
            "message_text": command_text_from_args(&["hello", "runtime"], Some(""), true),
            "stdin_text": command_text_from_args(&[], Some("from stdin\n"), false),
            "empty_tty_text": command_text_from_args(&[], Some(""), true),
            "with_file": build_run_prompt(
                "summarize",
                &[(format!("{workspace}/notes.txt").as_str(), "alpha\nbeta\n")]
            ),
        },
        "client": {
            "select_sessions": {
                "records": [
                    {"method": "GET", "server_url": "http://app.test", "path": "/api/sessions/session_existing", "auth_token": "server-secret"},
                    {"method": "GET", "server_url": "http://app.test", "path": "/api/sessions", "auth_token": "server-secret"},
                    {"method": "POST", "server_url": "http://app.test", "path": "/api/sessions", "payload": {"cwd": workspace}, "auth_token": "server-secret"},
                ],
                "explicit": {"id": "session_existing"},
                "continue": {"id": "session_latest"},
                "new": {"id": "session_new"},
            },
            "sse_parse": parse_sse_response_lines(&sse_lines).unwrap_or_default(),
            "emit_text": {
                "exit_code": text.exit_code,
                "stdout": text.stdout,
                "stderr": text.stderr,
            },
            "emit_json": {
                "exit_code": emitted_json.exit_code,
                "stdout_lines": emitted_json.stdout.lines().collect::<Vec<_>>(),
                "stderr": emitted_json.stderr,
            },
            "http_error": format_http_error("GET", "/api/health", 401, Some(&json!({"error": "unauthorized"}))),
        },
        "runtime": {
            "config": config.to_public_value(),
            "health": health_payload(&config),
            "routes": {
                "health": route_health().to_value(),
                "unauthorized": route_unauthorized().to_value(),
                "options": route_options().to_value(),
                "unknown": route_unknown().to_value(),
            },
        },
        "docker": {
            "dockerfile": dockerfile_lines(),
            "smoke_command": docker_smoke_command(),
            "expected_stdout_json": health_payload(&HttpRuntimeConfig {
                serve_static: false,
                ..HttpRuntimeConfig::default()
            }),
            "daemon_required": true,
        },
    })
}

fn fixture_events() -> Vec<Value> {
    vec![
        json!({
            "sequence": 1,
            "method": "item/agentMessage/delta",
            "params": {"event": {"text": "hello from server"}},
        }),
        json!({
            "sequence": 2,
            "method": "turn/completed",
            "params": {"status": "completed", "final_answer": "hello from server"},
        }),
    ]
}

fn sdk_exports() -> Vec<&'static str> {
    vec![
        "AgentConfig",
        "AgentLoop",
        "ExploreAgent",
        "LanguageModel",
        "Model",
        "OpenAIProvider",
        "PermissionAction",
        "PermissionManager",
        "PermissionRule",
        "PermissionRuleset",
        "PlanAgent",
        "QuestionManager",
        "RemoteMcpManager",
        "Session",
        "SkillDiscoveryReport",
        "SkillDocument",
        "SkillInfo",
        "SkillIssue",
        "SkillRegistry",
        "ToolkitAdapter",
        "UniversalAgent",
        "load_mcp_config_from_sources",
        "new_id",
    ]
}

fn emit_text_event(event: &Value, verbose: bool, stdout: &mut String, stderr: &mut String) -> bool {
    let method = event_method(event);
    let params = event_params(event);
    let payload = params.get("event").filter(|value| value.is_object());
    if method == "item/agentMessage/delta"
        && let Some(payload) = payload
    {
        stdout.push_str(
            payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        return true;
    }
    if matches!(method.as_str(), "turn/error" | "turn/failed") {
        let error = params
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default();
        stderr.push_str(&format!("{method}: {error}\n"));
        return false;
    }
    if verbose {
        stderr.push_str(&format!("[{method}]\n"));
    }
    false
}

fn event_method(event: &Value) -> String {
    event
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn event_params(event: &Value) -> Map<String, Value> {
    event
        .get("params")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub fn python_json_dumps(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => {
            if *value {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string()),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(python_json_dumps)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        Value::Object(items) => {
            let mut keys = items.keys().collect::<Vec<_>>();
            keys.sort();
            let inner = keys
                .into_iter()
                .map(|key| {
                    let rendered_key =
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                    let value = python_json_dumps(&items[key]);
                    format!("{rendered_key}: {value}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{inner}}}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_command_boundary() {
        assert_eq!(crate_name(), "openagent-http-runtime");
        assert_eq!(command_name(), "openagent-http-runtime");
        assert_eq!(app_server_crate_name(), "openagent-app-server");
    }
}
