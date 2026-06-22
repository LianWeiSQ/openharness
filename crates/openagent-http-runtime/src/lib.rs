//! HTTP runtime service contracts for the Rust rewrite.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use openagent_app_server::{
    approval_response_payload, control_next_payload, parse_turn_approval_path,
    parse_turn_question_reply_path, question_dismiss_payload, question_reply_payload,
    record_control_response_payload, tui_control_request_for_path,
};
use openagent_protocol::{ChatMessage, PermissionRuleset, Role, ToolCall, ToolResult, Usage};
use openagent_provider::{
    OpenAiLanguageModelConfig, ProviderStreamEvent, build_openai_chat_payload,
    build_openai_responses_payload, default_env_mapping, normalize_openai_chat_sse_chunks,
    normalize_openai_responses_response, normalize_openai_responses_stream_events,
    normalize_provider, parse_tool_arguments, provider_default_base_url, provider_default_model,
    provider_requires_api_key, summarize_http_error_body,
};
use openagent_session::{
    FileSessionStore, Session, SessionEventOptions, SessionPartOptions, SessionStatus,
    StartRunOptions,
};
use openagent_tools::{ToolContext, Toolkit, resolve_path_in_root};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 8787;
const INDEX_HTML: &str = include_str!("../../../src/openagent/app_server/static/index.html");
const APP_JS: &str = include_str!("../../../src/openagent/app_server/static/app.js");
const APP_CSS: &str = include_str!("../../../src/openagent/app_server/static/app.css");
const APP_EVENTS_FILE: &str = "app_events.jsonl";
const TUI_CONTROL_QUEUE_FILE: &str = "tui_control_queue.json";
const TUI_CONTROL_RESPONSES_FILE: &str = "tui_control_responses.jsonl";
const FILE_CHANGE_UNDO_STACK_KEY: &str = "file_change_undo_stack";
const FILE_CHANGE_REDO_STACK_KEY: &str = "file_change_redo_stack";
const FILE_CHANGE_LATEST_KEY: &str = "latest_file_change";
const MAX_FILE_CHANGE_STACK: usize = 50;
const MAX_RENDERED_DIFF_LINES: usize = 400;

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
    pub auth_username: Option<String>,
    pub auth_password: Option<String>,
    pub cors_origin: String,
    pub mdns_name: Option<String>,
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
            auth_username: None,
            auth_password: None,
            cors_origin: "*".to_string(),
            mdns_name: Some("openagent".to_string()),
        }
    }
}

impl HttpRuntimeConfig {
    #[must_use]
    pub fn auth_required(&self) -> bool {
        self.auth_token
            .as_ref()
            .is_some_and(|token| !token.is_empty())
            || self
                .auth_password
                .as_ref()
                .is_some_and(|password| !password.is_empty())
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
            "auth_basic_enabled": self.auth_password.as_ref().is_some_and(|value| !value.is_empty()),
            "cors_origin": self.cors_origin,
            "mdns_name": self.mdns_name,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
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
        body_text: None,
    }
}

#[must_use]
pub fn route_unauthorized() -> HttpResponseSpec {
    let mut headers = Map::new();
    headers.insert(
        "WWW-Authenticate".to_string(),
        Value::String(
            "Bearer realm=\"openagent-app-bridge\", Basic realm=\"openagent-app-bridge\""
                .to_string(),
        ),
    );
    HttpResponseSpec {
        status: 401,
        content_type: None,
        headers,
        body: Some(json!({"error": "unauthorized"})),
        body_text: None,
    }
}

#[must_use]
pub fn route_options() -> HttpResponseSpec {
    let mut headers = Map::new();
    headers.insert(
        "Access-Control-Allow-Methods".to_string(),
        Value::String("GET, POST, PATCH, DELETE, OPTIONS".to_string()),
    );
    headers.insert(
        "Access-Control-Allow-Headers".to_string(),
        Value::String("Authorization, Content-Type, X-OpenAgent-Token".to_string()),
    );
    headers.insert(
        "Access-Control-Max-Age".to_string(),
        Value::String("600".to_string()),
    );
    HttpResponseSpec {
        status: 204,
        content_type: None,
        headers,
        body: None,
        body_text: None,
    }
}

#[must_use]
pub fn route_unknown() -> HttpResponseSpec {
    HttpResponseSpec {
        status: 404,
        content_type: None,
        headers: Map::new(),
        body: Some(json!({"error": "unknown endpoint"})),
        body_text: None,
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
            "--username" | "-u" => {
                if let Some(value) = args.get(index + 1) {
                    config.auth_username = Some(value.clone());
                    index += 1;
                }
            }
            "--password" | "-p" => {
                if let Some(value) = args.get(index + 1) {
                    config.auth_password = Some(value.clone());
                    index += 1;
                }
            }
            "--cors-origin" => {
                if let Some(value) = args.get(index + 1) {
                    config.cors_origin = value.clone();
                    index += 1;
                }
            }
            "--mdns-name" => {
                if let Some(value) = args.get(index + 1) {
                    config.mdns_name = Some(value.clone());
                    index += 1;
                }
            }
            "--no-mdns" => {
                config.mdns_name = None;
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
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        return CliRunResult {
            exit_code: 0,
            stdout: "Usage: openagent-http-runtime [--host <host>] [--port <port>] [--workspace <path>] [--session-root <path>] [--headless] [--auth-token <token>] [-u|--username <name>] [-p|--password <password>] [--cors-origin <origin>] [--mdns-name <name>] [--no-mdns] [--health-json]\n".to_string(),
            stderr: String::new(),
        };
    }
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
    serve_blocking(config)
}

fn serve_blocking(config: HttpRuntimeConfig) -> CliRunResult {
    let listener = match TcpListener::bind((config.host.as_str(), config.port)) {
        Ok(listener) => listener,
        Err(error) => {
            return CliRunResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: format!("failed to bind HTTP runtime: {error}\n"),
            };
        }
    };
    let local = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| format!("{}:{}", config.host, config.port));
    println!("openagent HTTP runtime listening on http://{local}");
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let config = config.clone();
                thread::spawn(move || {
                    let _ = handle_http_stream(&mut stream, &config);
                });
            }
            Err(error) => eprintln!("openagent HTTP runtime accept failed: {error}"),
        }
    }
    CliRunResult {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }
}

fn handle_http_stream(stream: &mut TcpStream, config: &HttpRuntimeConfig) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let request = read_http_request(stream)?;
    if should_live_sse(&request, config) {
        return write_live_sse_response(stream, config, &request);
    }
    let response = route_http_request(&request, config);
    write_http_response(stream, with_runtime_headers(response, config))
}

#[derive(Clone, Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: String,
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 1024 * 1024 {
            return Err("request headers too large".to_string());
        }
    }
    let split = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .ok_or_else(|| "invalid HTTP request".to_string())?;
    let head = String::from_utf8_lossy(&buffer[..split]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let mut body_bytes = buffer[split..].to_vec();
    while body_bytes.len() < content_length {
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        body_bytes.extend_from_slice(&chunk[..read]);
    }
    body_bytes.truncate(content_length);
    Ok(HttpRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body_bytes).to_string(),
    })
}

fn route_http_request(request: &HttpRequest, config: &HttpRuntimeConfig) -> HttpResponseSpec {
    if request.method == "OPTIONS" {
        return route_options();
    }
    if !authorized(request, config) {
        return route_unauthorized();
    }
    let path = request.path.split('?').next().unwrap_or("/");
    match (request.method.as_str(), path) {
        ("GET", "/api/health") => json_response(200, health_payload(config)),
        ("GET", "/api/models") => json_response(200, models_payload()),
        ("GET", "/api/agents") => json_response(200, agents_payload()),
        ("GET", "/api/mdns") => json_response(200, mdns_payload(config)),
        ("GET", "/api/events") => sse_response(global_sse_frames(config, &request.path)),
        ("GET", "/api/sessions") => {
            json_response(200, list_sessions_payload(config, &request.path))
        }
        ("POST", "/api/sessions") => {
            json_response(200, create_session_payload(config, &request.body))
        }
        ("GET", "/") if config.serve_static => {
            static_response("text/html; charset=utf-8", INDEX_HTML)
        }
        ("GET", "/index.html") if config.serve_static => {
            static_response("text/html; charset=utf-8", INDEX_HTML)
        }
        ("GET", "/app.js") if config.serve_static => {
            static_response("application/javascript; charset=utf-8", APP_JS)
        }
        ("GET", "/app.css") if config.serve_static => {
            static_response("text/css; charset=utf-8", APP_CSS)
        }
        _ => route_dynamic_request(request, config, path),
    }
}

fn route_dynamic_request(
    request: &HttpRequest,
    config: &HttpRuntimeConfig,
    path: &str,
) -> HttpResponseSpec {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.len() == 3 && parts[0] == "api" && parts[1] == "sessions" {
        return match request.method.as_str() {
            "GET" => json_response(200, get_session_payload(config, parts[2])),
            "PATCH" => match update_session_payload(config, parts[2], &request.body) {
                Ok(payload) => json_response(200, payload),
                Err(error) => json_response(400, json!({"error": error})),
            },
            "DELETE" => match delete_session_payload(config, parts[2]) {
                Ok(payload) => json_response(200, payload),
                Err(error) => json_response(400, json!({"error": error})),
            },
            _ => route_unknown(),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "children"
        && request.method == "GET"
    {
        return json_response(200, session_children_payload(config, parts[2]));
    }
    if parts.len() == 4 && parts[0] == "api" && parts[1] == "sessions" && parts[3] == "share" {
        return match request.method.as_str() {
            "POST" => match share_session_payload(config, parts[2]) {
                Ok(payload) => json_response(200, payload),
                Err(error) => json_response(400, json!({"error": error})),
            },
            "DELETE" => match unshare_session_payload(config, parts[2]) {
                Ok(payload) => json_response(200, payload),
                Err(error) => json_response(400, json!({"error": error})),
            },
            _ => route_unknown(),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "compact"
        && request.method == "POST"
    {
        return match compact_session_payload(config, parts[2]) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 4 && parts[0] == "api" && parts[1] == "sessions" && parts[3] == "diff" {
        return match request.method.as_str() {
            "GET" => match session_diff_payload(config, parts[2]) {
                Ok(payload) => json_response(200, payload),
                Err(error) => json_response(400, json!({"error": error})),
            },
            _ => route_unknown(),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "undo"
        && request.method == "POST"
    {
        return match undo_session_payload(config, parts[2]) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "redo"
        && request.method == "POST"
    {
        return match redo_session_payload(config, parts[2]) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "turns"
        && request.method == "POST"
    {
        return match start_turn_payload(config, parts[2], &request.body) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "turns"
        && parts[3] == "events"
        && request.method == "GET"
    {
        return sse_response(turn_sse_frames(config, parts[2], &request.path));
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "turns"
        && parts[3] == "interrupt"
        && request.method == "POST"
    {
        return match interrupt_turn_payload(config, parts[2]) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if path.starts_with("/api/turns/") && path.contains("/approvals/") && request.method == "POST" {
        return match respond_approval_payload(config, path, &request.body) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if path.starts_with("/api/turns/")
        && path.contains("/questions/")
        && path.ends_with("/reply")
        && request.method == "POST"
    {
        return match respond_question_payload(config, path, &request.body) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if path == "/tui/control/next" && request.method == "GET" {
        return json_response(200, pop_tui_control_payload(config));
    }
    if path == "/tui/control/response" && request.method == "POST" {
        return json_response(200, record_tui_control_response(config, &request.body));
    }
    if path.starts_with("/tui/") && request.method == "POST" {
        return match enqueue_tui_control_payload(config, path, &request.body) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    route_unknown()
}

fn authorized(request: &HttpRequest, config: &HttpRuntimeConfig) -> bool {
    if !config.auth_required() {
        return true;
    }
    if let Some(token) = config.auth_token.as_ref().filter(|token| !token.is_empty()) {
        let bearer_ok = request
            .headers
            .get("authorization")
            .is_some_and(|value| value == &format!("Bearer {token}"));
        let header_ok = request
            .headers
            .get("x-openagent-token")
            .is_some_and(|value| value == token);
        if bearer_ok || header_ok {
            return true;
        }
    }
    basic_auth_ok(
        request.headers.get("authorization").map(String::as_str),
        config,
    )
}

fn json_response(status: u16, body: Value) -> HttpResponseSpec {
    HttpResponseSpec {
        status,
        content_type: Some("application/json; charset=utf-8".to_string()),
        headers: Map::new(),
        body: Some(body),
        body_text: None,
    }
}

fn static_response(content_type: &str, body: &str) -> HttpResponseSpec {
    HttpResponseSpec {
        status: 200,
        content_type: Some(content_type.to_string()),
        headers: Map::new(),
        body: None,
        body_text: Some(body.to_string()),
    }
}

fn sse_response(body: String) -> HttpResponseSpec {
    let mut headers = Map::new();
    headers.insert("Cache-Control".to_string(), json!("no-cache"));
    headers.insert("X-Accel-Buffering".to_string(), json!("no"));
    HttpResponseSpec {
        status: 200,
        content_type: Some("text/event-stream; charset=utf-8".to_string()),
        headers,
        body: None,
        body_text: Some(body),
    }
}

fn should_live_sse(request: &HttpRequest, config: &HttpRuntimeConfig) -> bool {
    if request.method != "GET" || !authorized(request, config) {
        return false;
    }
    let path = request.path.split('?').next().unwrap_or("/");
    let is_sse_path = path == "/api/events" || turn_id_from_events_path(path).is_some();
    is_sse_path
        && request
            .headers
            .get("accept")
            .is_some_and(|value| value.contains("text/event-stream"))
}

fn write_live_sse_response(
    stream: &mut TcpStream,
    config: &HttpRuntimeConfig,
    request: &HttpRequest,
) -> Result<(), String> {
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let path = request.path.split('?').next().unwrap_or("/");
    let turn_id = turn_id_from_events_path(path);
    let headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\ncache-control: no-cache, no-transform\r\nx-accel-buffering: no\r\naccess-control-allow-origin: {}\r\nconnection: close\r\n\r\n",
        config.cors_origin
    );
    stream
        .write_all(headers.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut last_id = last_event_id_from_path(&request.path);
    let timeout = live_sse_timeout(&request.path);
    let started = Instant::now();
    let mut last_heartbeat = Instant::now();
    loop {
        let mut terminal_seen = false;
        for (id, event) in live_sse_events_after(config, turn_id.as_deref(), last_id) {
            stream
                .write_all(sse_frame(id, &event).as_bytes())
                .map_err(|error| error.to_string())?;
            last_id = id;
            if is_terminal_turn_event(&event) {
                terminal_seen = true;
            }
        }
        stream.flush().map_err(|error| error.to_string())?;
        if terminal_seen {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            stream
                .write_all(b": ping\n\n")
                .map_err(|error| error.to_string())?;
            stream.flush().map_err(|error| error.to_string())?;
            return Ok(());
        }
        if last_heartbeat.elapsed() >= Duration::from_secs(10) {
            stream
                .write_all(b": ping\n\n")
                .map_err(|error| error.to_string())?;
            stream.flush().map_err(|error| error.to_string())?;
            last_heartbeat = Instant::now();
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn live_sse_events_after(
    config: &HttpRuntimeConfig,
    turn_id: Option<&str>,
    last_id: u64,
) -> Vec<(u64, Value)> {
    let events = if let Some(turn_id) = turn_id {
        turn_app_events(config, turn_id)
    } else {
        all_app_events(config)
    };
    events
        .into_iter()
        .enumerate()
        .filter_map(|(index, event)| {
            let id = event
                .get(if turn_id.is_some() {
                    "sequence"
                } else {
                    "global_sequence"
                })
                .or_else(|| event.get("sequence"))
                .and_then(Value::as_u64)
                .unwrap_or(index as u64 + 1);
            (id > last_id).then_some((id, event))
        })
        .collect()
}

fn turn_id_from_events_path(path: &str) -> Option<String> {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    (parts.len() == 4 && parts[0] == "api" && parts[1] == "turns" && parts[3] == "events")
        .then(|| parts[2].to_string())
}

fn live_sse_timeout(request_path: &str) -> Duration {
    let millis = query_value(request_path, "live_timeout_ms")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30_000)
        .clamp(250, 300_000);
    Duration::from_millis(millis)
}

fn is_terminal_turn_event(event: &Value) -> bool {
    matches!(
        event.get("method").and_then(Value::as_str),
        Some("turn/completed" | "turn/failed" | "turn/interrupted")
    )
}

fn with_runtime_headers(
    mut response: HttpResponseSpec,
    config: &HttpRuntimeConfig,
) -> HttpResponseSpec {
    response.headers.insert(
        "Access-Control-Allow-Origin".to_string(),
        json!(config.cors_origin.clone()),
    );
    response.headers.insert(
        "Access-Control-Allow-Headers".to_string(),
        json!("Authorization, Content-Type, X-OpenAgent-Token"),
    );
    response.headers.insert(
        "Access-Control-Allow-Methods".to_string(),
        json!("GET, POST, PATCH, DELETE, OPTIONS"),
    );
    response
}

fn write_http_response(stream: &mut TcpStream, response: HttpResponseSpec) -> Result<(), String> {
    let body = response.body_text.unwrap_or_else(|| {
        response
            .body
            .as_ref()
            .map(python_json_dumps)
            .unwrap_or_default()
    });
    let content_type = response
        .content_type
        .unwrap_or_else(|| "application/json; charset=utf-8".to_string());
    let status_text = match response.status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "OK",
    };
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n",
        response.status,
        status_text,
        content_type,
        body.len()
    );
    for (key, value) in response.headers {
        if let Some(value) = value.as_str() {
            headers.push_str(&format!("{key}: {value}\r\n"));
        }
    }
    headers.push_str("\r\n");
    stream
        .write_all(headers.as_bytes())
        .and_then(|()| stream.write_all(body.as_bytes()))
        .map_err(|error| error.to_string())
}

fn basic_auth_ok(authorization: Option<&str>, config: &HttpRuntimeConfig) -> bool {
    let Some(password) = config
        .auth_password
        .as_ref()
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let username = config.auth_username.as_deref().unwrap_or("openagent");
    let Some(encoded) = authorization.and_then(|value| value.strip_prefix("Basic ")) else {
        return false;
    };
    decode_base64(encoded).is_some_and(|decoded| decoded == format!("{username}:{password}"))
}

fn decode_base64(value: &str) -> Option<String> {
    let mut output = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u8;
    for byte in value.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let sextet = base64_value(byte)? as u32;
        buffer = (buffer << 6) | sextet;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    String::from_utf8(output).ok()
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn list_sessions_payload(config: &HttpRuntimeConfig, request_path: &str) -> Value {
    let root = session_root(config);
    let query = query_param(request_path, "query").unwrap_or_default();
    let mut sessions = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let state = read_json_file(&path.join("state.latest.json"));
            if state.as_object().is_none_or(Map::is_empty) {
                continue;
            }
            let summary = session_summary_from_state(&state, &entry.file_name().to_string_lossy());
            if !query.is_empty() && !session_matches_query(&summary, &query) {
                continue;
            }
            sessions.push(summary);
        }
    }
    sessions.sort_by(|left, right| {
        right["updated_at_ms"]
            .as_u64()
            .cmp(&left["updated_at_ms"].as_u64())
    });
    json!({"session_root": root.to_string_lossy(), "query": query, "sessions": sessions})
}

fn models_payload() -> Value {
    let current = default_model_id();
    let mut models = vec![json!({
        "id": current,
        "provider_id": "openagent",
        "name": "OpenAgent Server Local",
        "capabilities": {"tools": true, "streaming": true, "reasoning": true},
        "default": true,
    })];
    if models[0]["id"] != "server-local" {
        models.push(json!({
            "id": "server-local",
            "provider_id": "openagent",
            "name": "OpenAgent Server Local",
            "capabilities": {"tools": true, "streaming": true, "reasoning": true},
        }));
    }
    json!({
        "models": models,
        "variants": ["default", "fast", "balanced", "deep"],
        "thinking": ["off", "low", "medium", "high"],
    })
}

fn agents_payload() -> Value {
    json!({
        "agents": [
            {
                "id": "server",
                "name": "Server",
                "description": "Default server-backed coding agent",
                "default": true,
            },
            {
                "id": "coder",
                "name": "Coder",
                "description": "Implementation-focused profile",
            },
            {
                "id": "reviewer",
                "name": "Reviewer",
                "description": "Review and risk-focused profile",
            },
            {
                "id": "planner",
                "name": "Planner",
                "description": "Plan-first profile for large changes",
            }
        ],
    })
}

fn default_model_id() -> String {
    std::env::var("OPENAGENT_MODEL").unwrap_or_else(|_| "server-local".to_string())
}

fn mdns_payload(config: &HttpRuntimeConfig) -> Value {
    json!({
        "enabled": config.mdns_name.as_ref().is_some_and(|value| !value.is_empty()),
        "service": "_openagent._tcp",
        "name": config.mdns_name.clone().unwrap_or_default(),
        "host": config.host,
        "port": config.port,
        "url": format!("http://{}:{}", config.host, config.port),
    })
}

fn create_session_payload(config: &HttpRuntimeConfig, body: &str) -> Value {
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let workspace = payload
        .get("cwd")
        .or_else(|| payload.get("workspace"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| config.workspace.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let session_id = new_id("session");
    let store = FileSessionStore::new(session_root(config));
    let mut session = if let Some(fork_from) = payload.get("fork_from").and_then(Value::as_str) {
        store.load_session(fork_from).map_or_else(
            |_| Session::new(session_id.clone(), workspace.clone()),
            |base| {
                let mut forked = Session::new(session_id.clone(), workspace.clone());
                forked.messages = base.messages;
                forked.todos = base.todos;
                forked.metadata = base.metadata;
                forked
                    .metadata
                    .insert("forked_from".to_string(), json!(fork_from));
                forked
                    .metadata
                    .insert("parent_session_id".to_string(), json!(fork_from));
                forked
            },
        )
    } else {
        Session::new(session_id.clone(), workspace.clone())
    };
    session
        .metadata
        .insert("created_by".to_string(), json!("openagent-http-runtime"));
    if let Some(title) = payload
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        session
            .metadata
            .insert("title".to_string(), json!(title.trim()));
    }
    let _ = store.save_state(&session, None);
    json!({
        "session_id": session_id,
        "status": "created",
        "session": {
            "id": session_id,
            "session_id": session_id,
            "status": "idle",
            "message_count": 0,
            "workspace": workspace.to_string_lossy(),
        }
    })
}

fn get_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Value {
    let store = FileSessionStore::new(session_root(config));
    match store.load_session(session_id) {
        Ok(session) => json!({
            "session_id": session.id,
            "session": {
                "id": session.id,
                "session_id": session.id,
                "workspace": session.directory.to_string_lossy(),
                "status": session_status_text(&session.status),
                "message_count": session.messages.len(),
                "metadata": session.metadata,
            },
            "workspace": session.directory.to_string_lossy(),
            "status": session_status_text(&session.status),
            "message_count": session.messages.len(),
            "metadata": session.metadata,
        }),
        Err(error) => json!({"error": error.to_string()}),
    }
}

fn update_session_payload(
    config: &HttpRuntimeConfig,
    session_id: &str,
    body: &str,
) -> Result<Value, String> {
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    if let Some(title) = payload.get("title").and_then(Value::as_str) {
        let title = title.trim();
        if title.is_empty() {
            session.metadata.remove("title");
        } else {
            session.metadata.insert("title".to_string(), json!(title));
        }
    }
    if let Some(archived) = payload.get("archived").and_then(Value::as_bool) {
        if archived {
            session.metadata.insert("archived".to_string(), json!(true));
            session
                .metadata
                .insert("archived_at_ms".to_string(), json!(now_ms()));
        } else {
            session.metadata.remove("archived");
            session.metadata.remove("archived_at_ms");
        }
    }
    set_session_text_metadata(&mut session, &payload, "agent");
    set_session_text_metadata(&mut session, &payload, "model");
    set_session_text_metadata(&mut session, &payload, "variant");
    set_session_text_metadata(&mut session, &payload, "thinking");
    store
        .save_state(&session, None)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "session_id": session.id,
        "updated": true,
        "session": session_summary_from_session(&session),
    }))
}

fn delete_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    if !valid_session_id(session_id) {
        return Err("invalid session id".to_string());
    }
    let target = session_root(config).join(session_id);
    let removed = if target.exists() {
        fs::remove_dir_all(&target).map_err(|error| error.to_string())?;
        true
    } else {
        false
    };
    Ok(json!({"session_id": session_id, "removed": removed}))
}

fn session_children_payload(config: &HttpRuntimeConfig, session_id: &str) -> Value {
    let root = session_root(config);
    let mut children = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let state = read_json_file(&path.join("state.latest.json"));
            let parent = state
                .get("metadata")
                .and_then(|metadata| {
                    metadata
                        .get("parent_session_id")
                        .or_else(|| metadata.get("forked_from"))
                })
                .and_then(Value::as_str)
                .unwrap_or_default();
            if parent == session_id {
                children.push(session_summary_from_state(
                    &state,
                    &entry.file_name().to_string_lossy(),
                ));
            }
        }
    }
    children.sort_by(|left, right| {
        right["updated_at_ms"]
            .as_u64()
            .cmp(&left["updated_at_ms"].as_u64())
    });
    json!({"session_id": session_id, "children": children})
}

fn share_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    let share_id = session
        .metadata
        .get("share_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| new_id("share"));
    let url = format!("openagent://share/{share_id}");
    session.metadata.insert("shared".to_string(), json!(true));
    session
        .metadata
        .insert("share_id".to_string(), json!(share_id));
    session.metadata.insert("share_url".to_string(), json!(url));
    session
        .metadata
        .insert("shared_at_ms".to_string(), json!(now_ms()));
    store
        .save_state(&session, None)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "session_id": session.id,
        "shared": true,
        "share_id": session.metadata.get("share_id").cloned().unwrap_or(Value::Null),
        "url": session.metadata.get("share_url").cloned().unwrap_or(Value::Null),
    }))
}

fn unshare_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    session.metadata.remove("shared");
    session.metadata.remove("share_id");
    session.metadata.remove("share_url");
    session.metadata.remove("shared_at_ms");
    store
        .save_state(&session, None)
        .map_err(|error| error.to_string())?;
    Ok(json!({"session_id": session.id, "shared": false}))
}

fn compact_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    if matches!(
        session.status,
        SessionStatus::Running | SessionStatus::Paused
    ) {
        return Err("session must be idle before compacting".to_string());
    }
    session.status = SessionStatus::Compacting;
    store
        .save_state(&session, None)
        .map_err(|error| error.to_string())?;
    let summary = summarize_session_messages(&session);
    session.status = SessionStatus::Idle;
    session.metadata.insert(
        "compact".to_string(),
        json!({
            "compacted_at_ms": now_ms(),
            "message_count": session.messages.len(),
            "summary": summary,
        }),
    );
    store
        .save_state(&session, None)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "session_id": session.id,
        "status": "compacted",
        "summary": session.metadata.get("compact").cloned().unwrap_or(Value::Null),
    }))
}

fn session_diff_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    let undo_stack = file_change_stack(&session, FILE_CHANGE_UNDO_STACK_KEY);
    let redo_stack = file_change_stack(&session, FILE_CHANGE_REDO_STACK_KEY);
    let patches = undo_stack
        .iter()
        .rev()
        .map(public_file_change)
        .collect::<Vec<_>>();
    let redo = redo_stack
        .iter()
        .rev()
        .map(public_file_change)
        .collect::<Vec<_>>();
    Ok(json!({
        "session_id": session.id,
        "undo_count": undo_stack.len(),
        "redo_count": redo_stack.len(),
        "latest": undo_stack.last().map(public_file_change).unwrap_or(Value::Null),
        "patches": patches,
        "redo": redo,
    }))
}

fn undo_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    let mut undo_stack = file_change_stack(&session, FILE_CHANGE_UNDO_STACK_KEY);
    let Some(change) = undo_stack.pop() else {
        return Err("nothing to undo".to_string());
    };
    apply_file_change_state(&session, &change, FileChangeState::Before)?;
    let mut redo_stack = file_change_stack(&session, FILE_CHANGE_REDO_STACK_KEY);
    let reverted = mark_file_change(change.clone(), "undone");
    push_stack_entry(&mut redo_stack, reverted.clone());
    set_file_change_stack(&mut session, FILE_CHANGE_UNDO_STACK_KEY, undo_stack.clone());
    set_file_change_stack(&mut session, FILE_CHANGE_REDO_STACK_KEY, redo_stack.clone());
    session.metadata.insert(
        "latest_file_revert".to_string(),
        public_file_change(&reverted),
    );
    let turn_id = file_change_run_id(&change);
    let public = public_file_change(&reverted);
    let event = append_patch_stack_event(&store, &session, &turn_id, "patch/undone", &public);
    store
        .save_state(&session, Some(&turn_id))
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "session_id": session.id,
        "status": "undone",
        "undo_count": undo_stack.len(),
        "redo_count": redo_stack.len(),
        "patch": public,
        "events": [event],
    }))
}

fn redo_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    let mut redo_stack = file_change_stack(&session, FILE_CHANGE_REDO_STACK_KEY);
    let Some(change) = redo_stack.pop() else {
        return Err("nothing to redo".to_string());
    };
    apply_file_change_state(&session, &change, FileChangeState::After)?;
    let mut undo_stack = file_change_stack(&session, FILE_CHANGE_UNDO_STACK_KEY);
    let reapplied = mark_file_change(change.clone(), "applied");
    push_stack_entry(&mut undo_stack, reapplied.clone());
    set_file_change_stack(&mut session, FILE_CHANGE_UNDO_STACK_KEY, undo_stack.clone());
    set_file_change_stack(&mut session, FILE_CHANGE_REDO_STACK_KEY, redo_stack.clone());
    session.metadata.insert(
        FILE_CHANGE_LATEST_KEY.to_string(),
        public_file_change(&reapplied),
    );
    let turn_id = file_change_run_id(&change);
    let public = public_file_change(&reapplied);
    let event = append_patch_stack_event(&store, &session, &turn_id, "patch/redone", &public);
    store
        .save_state(&session, Some(&turn_id))
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "session_id": session.id,
        "status": "redone",
        "undo_count": undo_stack.len(),
        "redo_count": redo_stack.len(),
        "patch": public,
        "events": [event],
    }))
}

#[derive(Clone, Debug)]
struct FileChangeBefore {
    target: PathBuf,
    display_path: String,
    existed_before: bool,
    before_content: Option<String>,
}

#[derive(Clone, Copy, Debug)]
enum FileChangeState {
    Before,
    After,
}

fn capture_file_change_before(session: &Session, call: &ToolCall) -> Option<FileChangeBefore> {
    if !matches!(call.name.as_str(), "write" | "edit") {
        return None;
    }
    let raw_path = call.input.get("file_path").and_then(Value::as_str)?;
    let target = resolve_path_in_root(&session.directory, raw_path).ok()?;
    let existed_before = target.exists();
    let before_content = if target.is_file() {
        fs::read_to_string(&target).ok()
    } else {
        None
    };
    Some(FileChangeBefore {
        display_path: session_display_path(session, &target),
        target,
        existed_before,
        before_content,
    })
}

fn file_change_preview(before: &FileChangeBefore, call: &ToolCall) -> Option<Value> {
    let after = predicted_after_content(before, call)?;
    let existed_after = true;
    let diff = render_unified_diff(
        &before.display_path,
        before.before_content.as_deref(),
        Some(after.as_str()),
    );
    Some(json!({
        "kind": "file",
        "path": before.display_path,
        "status": file_change_status(before.existed_before, existed_after),
        "diff": diff,
        "summary": format!(
            "{} {}",
            call.name,
            if before.existed_before { "will modify file" } else { "will create file" }
        ),
    }))
}

fn predicted_after_content(before: &FileChangeBefore, call: &ToolCall) -> Option<String> {
    match call.name.as_str() {
        "write" => call
            .input
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string),
        "edit" => {
            let old = call.input.get("old_string").and_then(Value::as_str)?;
            let new = call.input.get("new_string").and_then(Value::as_str)?;
            let replace_all = call
                .input
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if old.is_empty() {
                return Some(new.to_string());
            }
            preview_replace_text(before.before_content.as_deref()?, old, new, replace_all).ok()
        }
        _ => None,
    }
}

fn complete_file_change(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    call: &ToolCall,
    before: Option<FileChangeBefore>,
    result: &ToolResult,
) -> Option<Value> {
    if result.error.is_some() {
        return None;
    }
    let before = before?;
    let existed_after = before.target.exists();
    let after_content = if before.target.is_file() {
        fs::read_to_string(&before.target).ok()
    } else {
        None
    };
    if before.existed_before == existed_after && before.before_content == after_content {
        return None;
    }
    let diff = render_unified_diff(
        &before.display_path,
        before.before_content.as_deref(),
        after_content.as_deref(),
    );
    let change = json!({
        "id": new_id("patch"),
        "session_id": session.id,
        "run_id": run_id,
        "call_id": call.call_id,
        "tool": call.name,
        "created_at_ms": now_ms(),
        "workspace": session.directory.to_string_lossy(),
        "path": before.display_path,
        "absolute_path": before.target.to_string_lossy(),
        "existed_before": before.existed_before,
        "existed_after": existed_after,
        "before": before.before_content,
        "after": after_content,
        "status": "applied",
        "diff": diff,
    });
    push_file_change(session, change.clone());
    let public = public_file_change(&change);
    let _ = store.record_event(
        &session.id,
        run_id,
        "patch.detected",
        SessionEventOptions {
            kind: "patch".to_string(),
            attributes: BTreeMap::from([("patch".to_string(), public)]),
            ..SessionEventOptions::default()
        },
    );
    Some(change)
}

fn patch_detected_event(session: &Session, run_id: &str, change: &Value) -> Value {
    json!({
        "method": "patch/detected",
        "params": {
            "session_id": session.id,
            "thread_id": session.id,
            "turn_id": run_id,
            "patch": public_file_change(change),
        }
    })
}

fn append_patch_stack_event(
    store: &FileSessionStore,
    session: &Session,
    turn_id: &str,
    method: &str,
    patch: &Value,
) -> Value {
    let event_name = match method {
        "patch/undone" => "patch.undone",
        "patch/redone" => "patch.redone",
        _ => "patch.changed",
    };
    let event = json!({
        "method": method,
        "params": {
            "session_id": session.id,
            "thread_id": session.id,
            "turn_id": turn_id,
            "patch": patch,
        }
    });
    append_app_events(
        &store.root,
        &session.id,
        turn_id,
        std::slice::from_ref(&event),
    );
    let _ = store.record_event(
        &session.id,
        turn_id,
        event_name,
        SessionEventOptions {
            kind: "patch".to_string(),
            attributes: BTreeMap::from([("patch".to_string(), patch.clone())]),
            ..SessionEventOptions::default()
        },
    );
    event
}

fn push_file_change(session: &mut Session, change: Value) {
    let public = public_file_change(&change);
    let mut undo_stack = file_change_stack(session, FILE_CHANGE_UNDO_STACK_KEY);
    push_stack_entry(&mut undo_stack, change);
    set_file_change_stack(session, FILE_CHANGE_UNDO_STACK_KEY, undo_stack);
    set_file_change_stack(session, FILE_CHANGE_REDO_STACK_KEY, Vec::new());
    session
        .metadata
        .insert(FILE_CHANGE_LATEST_KEY.to_string(), public);
}

fn file_change_stack(session: &Session, key: &str) -> Vec<Value> {
    session
        .metadata
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn set_file_change_stack(session: &mut Session, key: &str, stack: Vec<Value>) {
    session
        .metadata
        .insert(key.to_string(), Value::Array(stack));
}

fn push_stack_entry(stack: &mut Vec<Value>, value: Value) {
    stack.push(value);
    let excess = stack.len().saturating_sub(MAX_FILE_CHANGE_STACK);
    if excess > 0 {
        stack.drain(0..excess);
    }
}

fn apply_file_change_state(
    session: &Session,
    change: &Value,
    state: FileChangeState,
) -> Result<(), String> {
    let path = change
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| change.get("absolute_path").and_then(Value::as_str))
        .ok_or_else(|| "patch is missing path".to_string())?;
    let target = resolve_path_in_root(&session.directory, path)?;
    let (exists_key, content_key) = match state {
        FileChangeState::Before => ("existed_before", "before"),
        FileChangeState::After => ("existed_after", "after"),
    };
    let should_exist = change
        .get(exists_key)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if should_exist {
        let content = change
            .get(content_key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("patch is missing {content_key} content"))?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(&target, content).map_err(|error| error.to_string())
    } else if target.exists() {
        if target.is_dir() {
            return Err(format!(
                "refusing to remove directory: {}",
                target.display()
            ));
        }
        fs::remove_file(&target).map_err(|error| error.to_string())
    } else {
        Ok(())
    }
}

fn mark_file_change(mut change: Value, status: &str) -> Value {
    if let Some(object) = change.as_object_mut() {
        object.insert("status".to_string(), json!(status));
        object.insert(format!("{status}_at_ms"), json!(now_ms()));
    }
    change
}

fn public_file_change(change: &Value) -> Value {
    let mut public = change.clone();
    if let Some(object) = public.as_object_mut() {
        object.remove("before");
        object.remove("after");
        object.remove("absolute_path");
    }
    public
}

fn file_change_run_id(change: &Value) -> String {
    change
        .get("run_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| new_id("turn"))
}

fn session_display_path(session: &Session, target: &Path) -> String {
    let root = session
        .directory
        .canonicalize()
        .unwrap_or_else(|_| session.directory.clone());
    target
        .strip_prefix(&root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/")
}

fn file_change_status(existed_before: bool, existed_after: bool) -> &'static str {
    match (existed_before, existed_after) {
        (false, true) => "created",
        (true, false) => "deleted",
        (true, true) => "modified",
        (false, false) => "unchanged",
    }
}

fn preview_replace_text(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, String> {
    if old == new {
        return Err("old_string and new_string must be different".to_string());
    }
    if old.is_empty() {
        return Ok(new.to_string());
    }
    let count = content.matches(old).count();
    if count == 0 {
        return Err("old_string not found in content".to_string());
    }
    if count > 1 && !replace_all {
        return Err("old_string found multiple times".to_string());
    }
    if replace_all {
        Ok(content.replace(old, new))
    } else {
        Ok(content.replacen(old, new, 1))
    }
}

fn render_unified_diff(path: &str, before: Option<&str>, after: Option<&str>) -> String {
    let before_lines = before
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let after_lines = after
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut lines = vec![
        format!("--- a/{path}"),
        format!("+++ b/{path}"),
        "@@".to_string(),
    ];
    let diff_lines = if before_lines.len().saturating_mul(after_lines.len()) <= 200_000 {
        lcs_diff_lines(&before_lines, &after_lines)
    } else {
        full_file_diff_lines(&before_lines, &after_lines)
    };
    lines.extend(diff_lines);
    truncate_diff_lines(lines).join("\n")
}

fn lcs_diff_lines(before: &[&str], after: &[&str]) -> Vec<String> {
    let rows = before.len() + 1;
    let cols = after.len() + 1;
    let mut table = vec![0_usize; rows * cols];
    for row in 1..rows {
        for col in 1..cols {
            table[row * cols + col] = if before[row - 1] == after[col - 1] {
                table[(row - 1) * cols + col - 1] + 1
            } else {
                table[(row - 1) * cols + col].max(table[row * cols + col - 1])
            };
        }
    }
    let mut row = before.len();
    let mut col = after.len();
    let mut output = Vec::new();
    while row > 0 || col > 0 {
        if row > 0 && col > 0 && before[row - 1] == after[col - 1] {
            output.push(format!(" {}", before[row - 1]));
            row -= 1;
            col -= 1;
        } else if col > 0
            && (row == 0 || table[row * cols + col - 1] >= table[(row - 1) * cols + col])
        {
            output.push(format!("+{}", after[col - 1]));
            col -= 1;
        } else if row > 0 {
            output.push(format!("-{}", before[row - 1]));
            row -= 1;
        }
    }
    output.reverse();
    output
}

fn full_file_diff_lines(before: &[&str], after: &[&str]) -> Vec<String> {
    before
        .iter()
        .map(|line| format!("-{line}"))
        .chain(after.iter().map(|line| format!("+{line}")))
        .collect()
}

fn truncate_diff_lines(mut lines: Vec<String>) -> Vec<String> {
    if lines.len() <= MAX_RENDERED_DIFF_LINES {
        return lines;
    }
    let omitted = lines.len() - MAX_RENDERED_DIFF_LINES;
    lines.truncate(MAX_RENDERED_DIFF_LINES);
    lines.push(format!("... diff truncated ({omitted} more lines) ..."));
    lines
}

fn query_param(path: &str, target: &str) -> Option<String> {
    path.split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(key, value)| (key == target).then(|| percent_decode(value)))
}

fn percent_decode(value: &str) -> String {
    let mut bytes = Vec::new();
    let raw = value.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' && index + 2 < raw.len() {
            if let (Some(high), Some(low)) = (hex_value(raw[index + 1]), hex_value(raw[index + 2]))
            {
                bytes.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        bytes.push(if raw[index] == b'+' { b' ' } else { raw[index] });
        index += 1;
    }
    String::from_utf8_lossy(&bytes).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn session_summary_from_session(session: &Session) -> Value {
    let metadata = serde_json::to_value(&session.metadata).unwrap_or_else(|_| json!({}));
    let title = metadata
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let archived = metadata
        .get("archived")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let shared = metadata
        .get("shared")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let share_url = metadata.get("share_url").cloned().unwrap_or(Value::Null);
    let forked_from = metadata.get("forked_from").cloned().unwrap_or(Value::Null);
    let parent_session_id = metadata
        .get("parent_session_id")
        .cloned()
        .unwrap_or(Value::Null);
    let compact = metadata.get("compact").cloned().unwrap_or(Value::Null);
    json!({
        "id": session.id.as_str(),
        "session_id": session.id.as_str(),
        "workspace": session.directory.to_string_lossy(),
        "status": session_status_text(&session.status),
        "updated_at_ms": now_ms(),
        "message_count": session.messages.len(),
        "metadata": metadata,
        "title": title,
        "archived": archived,
        "shared": shared,
        "share_url": share_url,
        "forked_from": forked_from,
        "parent_session_id": parent_session_id,
        "compact": compact,
    })
}

fn session_summary_from_state(state: &Value, fallback_id: &str) -> Value {
    let metadata = state
        .get("metadata")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let title = metadata
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let archived = metadata
        .get("archived")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let shared = metadata
        .get("shared")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let share_url = metadata.get("share_url").cloned().unwrap_or(Value::Null);
    let forked_from = metadata.get("forked_from").cloned().unwrap_or(Value::Null);
    let parent_session_id = metadata
        .get("parent_session_id")
        .cloned()
        .unwrap_or(Value::Null);
    let compact = metadata.get("compact").cloned().unwrap_or(Value::Null);
    json!({
        "id": state.get("session_id").cloned().unwrap_or_else(|| json!(fallback_id)),
        "session_id": state.get("session_id").cloned().unwrap_or_else(|| json!(fallback_id)),
        "workspace": state.get("workspace").cloned().unwrap_or_else(|| json!(".")),
        "status": state.get("status").cloned().unwrap_or_else(|| json!("idle")),
        "updated_at_ms": state.get("updated_at_ms").cloned().unwrap_or_else(|| json!(0)),
        "message_count": state.get("messages").and_then(Value::as_array).map_or(0, Vec::len),
        "metadata": metadata,
        "title": title,
        "archived": archived,
        "shared": shared,
        "share_url": share_url,
        "forked_from": forked_from,
        "parent_session_id": parent_session_id,
        "compact": compact,
    })
}

fn session_matches_query(summary: &Value, query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    [
        "session_id",
        "id",
        "title",
        "workspace",
        "status",
        "forked_from",
        "parent_session_id",
    ]
    .iter()
    .any(|key| {
        summary
            .get(*key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(&query)
    })
}

fn summarize_session_messages(session: &Session) -> String {
    let mut pieces = Vec::new();
    for message in session.messages.iter().take(12) {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
        };
        let text = message
            .content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        let truncated = if text.chars().count() > 160 {
            format!("{}...", text.chars().take(160).collect::<String>())
        } else {
            text
        };
        pieces.push(format!("{role}: {truncated}"));
    }
    if pieces.is_empty() {
        "No messages yet.".to_string()
    } else {
        pieces.join("\n")
    }
}

fn valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        && !session_id.contains("..")
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeProfile {
    agent: String,
    model: String,
    variant: String,
    thinking: String,
}

fn apply_turn_runtime_profile(session: &mut Session, payload: &Value) -> RuntimeProfile {
    set_session_text_metadata(session, payload, "agent");
    set_session_text_metadata(session, payload, "model");
    set_session_text_metadata(session, payload, "variant");
    set_session_text_metadata(session, payload, "thinking");
    let profile = RuntimeProfile {
        agent: session_text_metadata(session, "agent", "server"),
        model: session_text_metadata(session, "model", &default_model_id()),
        variant: session_text_metadata(session, "variant", "default"),
        thinking: session_text_metadata(session, "thinking", "medium"),
    };
    session
        .metadata
        .insert("agent".to_string(), json!(profile.agent.clone()));
    session
        .metadata
        .insert("model".to_string(), json!(profile.model.clone()));
    session
        .metadata
        .insert("variant".to_string(), json!(profile.variant.clone()));
    session
        .metadata
        .insert("thinking".to_string(), json!(profile.thinking.clone()));
    profile
}

fn set_session_text_metadata(session: &mut Session, payload: &Value, key: &str) {
    let Some(value) = payload.get(key).and_then(Value::as_str) else {
        return;
    };
    let value = value.trim();
    if value.is_empty() {
        session.metadata.remove(key);
    } else {
        session.metadata.insert(key.to_string(), json!(value));
    }
}

fn session_text_metadata(session: &Session, key: &str, default: &str) -> String {
    session
        .metadata
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| default.to_string())
}

fn turn_started_event(session: &Session, run_id: &str) -> Value {
    let profile = RuntimeProfile {
        agent: session_text_metadata(session, "agent", "server"),
        model: session_text_metadata(session, "model", &default_model_id()),
        variant: session_text_metadata(session, "variant", "default"),
        thinking: session_text_metadata(session, "thinking", "medium"),
    };
    json!({
        "method": "turn/started",
        "params": {
            "session_id": session.id,
            "thread_id": session.id,
            "turn_id": run_id,
            "status": "running",
            "agent": profile.agent,
            "agent_name": profile.agent,
            "model": profile.model,
            "model_id": profile.model,
            "provider_id": "openagent",
            "variant": profile.variant,
            "thinking": profile.thinking,
        },
    })
}

fn latest_user_message(session: &Session) -> String {
    session
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, Role::User))
        .map(|message| message.content.clone())
        .unwrap_or_default()
}

fn usage_payload(input: &str, output: &str, tool_calls: u64) -> Value {
    let input_tokens = estimate_tokens(input);
    let output_tokens = estimate_tokens(output);
    let tool_tokens = tool_calls.saturating_mul(16);
    let total_tokens = input_tokens + output_tokens + tool_tokens;
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "tool_tokens": tool_tokens,
        "total_tokens": total_tokens,
        "tool_calls": tool_calls,
        "cost": 0.0,
        "estimated": true,
    })
}

fn estimate_tokens(value: &str) -> u64 {
    let by_words = value.split_whitespace().count() as u64;
    let by_chars = (value.chars().count() as u64).div_ceil(4);
    by_words.max(by_chars).max(u64::from(!value.is_empty()))
}

fn trace_payload(session: &Session, run_id: &str, tool_calls: u64) -> Value {
    json!({
        "run_id": run_id,
        "session_id": session.id,
        "agent": session_text_metadata(session, "agent", "server"),
        "model": session_text_metadata(session, "model", &default_model_id()),
        "variant": session_text_metadata(session, "variant", "default"),
        "thinking": session_text_metadata(session, "thinking", "medium"),
        "tool_calls": tool_calls,
    })
}

fn record_usage_event(store: &FileSessionStore, session: &Session, run_id: &str, usage: &Value) {
    let _ = store.record_event(
        &session.id,
        run_id,
        "model.usage",
        SessionEventOptions {
            kind: "usage".to_string(),
            attributes: usage
                .as_object()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            ..SessionEventOptions::default()
        },
    );
}

fn tool_calls_completed_successfully(events: &[Value]) -> bool {
    events
        .iter()
        .any(|event| event.get("method").and_then(Value::as_str) == Some("item/toolCall/completed"))
        && !events.iter().any(|event| {
            event.get("method").and_then(Value::as_str) == Some("item/toolCall/failed")
        })
}

#[derive(Clone, Debug)]
struct RuntimeProviderResult {
    answer: String,
    tool_calls: Vec<ToolCall>,
    usage: Usage,
    source: String,
    finish_reason: String,
}

#[derive(Clone, Debug)]
struct RuntimeProviderLoopCarry {
    answer: String,
    usage: Usage,
    tool_calls: u64,
    next_step: u64,
}

impl Default for RuntimeProviderLoopCarry {
    fn default() -> Self {
        Self {
            answer: String::new(),
            usage: Usage::default(),
            tool_calls: 0,
            next_step: 1,
        }
    }
}

#[derive(Clone, Debug)]
struct RuntimeProviderResume {
    payload: Value,
    carry: RuntimeProviderLoopCarry,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
}

fn provider_turn_result(
    session: &Session,
    payload: &Value,
    stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
) -> Result<RuntimeProviderResult, String> {
    let provider_raw = payload
        .get("provider")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            session
                .metadata
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| std::env::var("OPENAGENT_PROVIDER").ok())
        .unwrap_or_else(|| "openai".to_string());
    let provider = normalize_provider(Some(&provider_raw))?;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            session
                .metadata
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| std::env::var("OPENAGENT_MODEL").ok())
        .or_else(|| provider_default_model(&provider).ok().flatten())
        .unwrap_or_else(|| "gpt-4o-mini".to_string());
    let env = default_env_mapping(&provider)?;
    let api_key = payload
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| env.get("api_key").and_then(|key| std::env::var(key).ok()))
        .or_else(|| std::env::var("OPENAGENT_API_KEY").ok());
    if provider_requires_api_key(&provider)? && api_key.as_deref().unwrap_or_default().is_empty() {
        return Ok(RuntimeProviderResult {
            answer: format!(
                "Provider `{provider}` is not configured. Set {} or OPENAGENT_API_KEY, then retry this turn.",
                env.get("api_key")
                    .map(String::as_str)
                    .unwrap_or("OPENAI_API_KEY")
            ),
            tool_calls: Vec::new(),
            usage: Usage::default(),
            source: "provider_missing_api_key".to_string(),
            finish_reason: "configuration_required".to_string(),
        });
    }
    let api_key = api_key.unwrap_or_default();
    let base_url = payload
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| env.get("base_url").and_then(|key| std::env::var(key).ok()))
        .or_else(|| provider_default_base_url(&provider).ok().flatten())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let wire_api = payload
        .get("wire_api")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| env.get("wire_api").and_then(|key| std::env::var(key).ok()))
        .unwrap_or_else(|| "responses".to_string());
    let timeout = payload
        .get("timeout_s")
        .and_then(Value::as_u64)
        .unwrap_or(60);
    let stream = provider_streaming_enabled_for_turn(payload);
    let tools = Toolkit::with_builtins().get_all_tools("local");
    call_openai_compatible_provider_for_runtime(
        &provider,
        &model,
        &api_key,
        &base_url,
        &wire_api,
        timeout,
        stream,
        &session.messages,
        &tools,
        stream_sink,
    )
}

fn call_openai_compatible_provider_for_runtime(
    provider: &str,
    model: &str,
    api_key: &str,
    base_url: &str,
    wire_api: &str,
    timeout_s: u64,
    stream: bool,
    messages: &[ChatMessage],
    tools: &[openagent_protocol::ToolSchema],
    mut stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
) -> Result<RuntimeProviderResult, String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(timeout_s.max(1)))
        .build()
        .map_err(|error| error.to_string())?;
    let mut config = OpenAiLanguageModelConfig::new(api_key, model);
    config.provider_id = provider.to_string();
    config.base_url = base_url.to_string();
    config.wire_api = wire_api.to_string();
    let (endpoint, payload) = if wire_api == "chat" {
        let mut payload =
            build_openai_chat_payload(&config, None, messages, tools, None, None, None);
        if let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(stream));
        }
        (join_url(base_url, "chat/completions"), payload)
    } else {
        let mut payload =
            build_openai_responses_payload(&config, None, messages, tools, None, None);
        if let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(stream));
        }
        (join_url(base_url, "responses"), payload)
    };
    let mut request = client
        .post(endpoint)
        .bearer_auth(api_key)
        .header("content-type", "application/json");
    if stream {
        request = request.header("accept", "text/event-stream");
    }
    let response = request
        .json(&payload)
        .send()
        .map_err(|error| format!("provider request failed: {error}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if stream && content_type.contains("text/event-stream") {
        if !status.is_success() {
            let raw = response
                .text()
                .map_err(|error| format!("provider response read failed: {error}"))?;
            return Err(format!(
                "provider returned HTTP {}: {}",
                status.as_u16(),
                summarize_http_error_body(&raw, &content_type)
            ));
        }
        let mut chunks = Vec::new();
        read_sse_json_values_stream(response, |chunk| {
            if let Some(event) = openai_stream_text_delta(wire_api, &chunk)
                && let Some(sink) = stream_sink.as_deref_mut()
            {
                sink(&event);
            }
            chunks.push(chunk);
            Ok(())
        })?;
        let events = if wire_api == "chat" {
            normalize_openai_chat_sse_chunks(&chunks)
        } else {
            normalize_openai_responses_stream_events(&chunks)
        };
        return Ok(provider_events_to_runtime_result(
            &events,
            format!("{provider}:{wire_api}:stream"),
            None,
        ));
    }
    let raw = response
        .text()
        .map_err(|error| format!("provider response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "provider returned HTTP {}: {}",
            status.as_u16(),
            summarize_http_error_body(&raw, &content_type)
        ));
    }
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("provider response was not JSON: {error}"))?;
    if wire_api == "chat" {
        Ok(openai_chat_response_to_runtime_result(
            &value,
            format!("{provider}:chat"),
        ))
    } else {
        let events = normalize_openai_responses_response(&value);
        Ok(provider_events_to_runtime_result(
            &events,
            format!("{provider}:responses"),
            Some(&value),
        ))
    }
}

fn read_sse_json_values_stream<R, F>(mut reader: R, mut on_value: F) -> Result<(), String>
where
    R: Read,
    F: FnMut(Value) -> Result<(), String>,
{
    let mut raw = String::new();
    let mut buffer = [0_u8; 4096];
    let mut saw_done = false;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(_error) if saw_done => break,
            Err(error) => return Err(format!("provider SSE read failed: {error}")),
        };
        if read == 0 {
            break;
        }
        raw.push_str(&String::from_utf8_lossy(&buffer[..read]));
        while let Some(index) = sse_frame_end(&raw) {
            let frame = raw[..index].to_string();
            let drain_to = if raw[index..].starts_with("\r\n\r\n") {
                index + 4
            } else {
                index + 2
            };
            raw.drain(..drain_to);
            if sse_frame_is_done(&frame) {
                saw_done = true;
            }
            if let Some(value) = parse_sse_frame_json(&frame)? {
                on_value(value)?;
            }
        }
    }
    if !raw.trim().is_empty()
        && let Some(value) = parse_sse_frame_json(&raw)?
    {
        on_value(value)?;
    }
    Ok(())
}

fn sse_frame_is_done(frame: &str) -> bool {
    frame.lines().any(|line| {
        let line = line.trim_end_matches('\r');
        line.strip_prefix("data:")
            .map(str::trim)
            .is_some_and(|data| data == "[DONE]")
    })
}

fn sse_frame_end(raw: &str) -> Option<usize> {
    match (raw.find("\r\n\r\n"), raw.find("\n\n")) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(index), None) | (None, Some(index)) => Some(index),
        (None, None) => None,
    }
}

fn parse_sse_frame_json(frame: &str) -> Result<Option<Value>, String> {
    let mut data_lines = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }
    if data_lines.is_empty() {
        return Ok(None);
    }
    let data = data_lines.join("\n");
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .map_err(|error| format!("provider SSE data was not JSON: {error}"))
}

fn openai_stream_text_delta(wire_api: &str, chunk: &Value) -> Option<ProviderStreamEvent> {
    let text = if wire_api == "chat" {
        chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("content"))
            .or_else(|| {
                chunk
                    .get("choices")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(|choice| choice.get("text"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else if matches!(
        chunk.get("type").and_then(Value::as_str),
        Some("response.output_text.delta" | "response.refusal.delta")
    ) {
        chunk
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else {
        ""
    };
    (!text.is_empty()).then(|| ProviderStreamEvent::TextDelta {
        text: text.to_string(),
    })
}

fn openai_chat_response_to_runtime_result(value: &Value, source: String) -> RuntimeProviderResult {
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));
    let answer = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let function = item.get("function")?;
            Some(ToolCall {
                call_id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("chat_tool_call_{index}"), str::to_string),
                name: function
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: parse_tool_arguments(
                    function
                        .get("arguments")
                        .unwrap_or(&Value::String(String::new())),
                ),
            })
        })
        .collect::<Vec<_>>();
    let usage = usage_from_provider_json(value.get("usage"));
    RuntimeProviderResult {
        answer: if answer.is_empty() && tool_calls.is_empty() {
            python_json_dumps(value)
        } else {
            answer
        },
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or(if tool_calls.is_empty() {
                "stop"
            } else {
                "tool_call"
            })
            .to_string(),
        tool_calls,
        usage,
        source,
    }
}

fn provider_events_to_runtime_result(
    events: &[ProviderStreamEvent],
    source: String,
    fallback: Option<&Value>,
) -> RuntimeProviderResult {
    let mut answer = String::new();
    let mut tool_calls = Vec::new();
    let mut usage = Usage::default();
    let mut finish_reason = "stop".to_string();
    for event in events {
        match event {
            ProviderStreamEvent::TextDelta { text } => answer.push_str(text),
            ProviderStreamEvent::ToolCall {
                call_id,
                name,
                input,
            } => tool_calls.push(ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                input: input.clone(),
            }),
            ProviderStreamEvent::Finish {
                usage: item,
                finish_reason: reason,
            } => {
                usage = item.clone();
                finish_reason = reason.clone();
            }
        }
    }
    if answer.is_empty()
        && tool_calls.is_empty()
        && let Some(value) = fallback
    {
        answer = python_json_dumps(value);
    }
    RuntimeProviderResult {
        answer,
        tool_calls,
        usage,
        source,
        finish_reason,
    }
}

fn provider_max_steps(payload: &Value) -> u64 {
    payload
        .get("max_steps")
        .or_else(|| payload.get("maxSteps"))
        .and_then(Value::as_u64)
        .unwrap_or(4)
        .clamp(1, 16)
}

fn add_usage(total: &mut Usage, item: &Usage) {
    total.input_tokens = total.input_tokens.saturating_add(item.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(item.output_tokens);
    total.cost += item.cost;
}

fn provider_resume_payload(payload: &Value) -> Value {
    let mut value = payload.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("input");
        object.remove("message");
        object.remove("tool_call");
        object.remove("tool_calls");
        object.remove("api_key");
    }
    value
}

fn store_pending_provider_turn(
    session: &mut Session,
    payload: &Value,
    carry: &RuntimeProviderLoopCarry,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
) {
    session.metadata.insert(
        "pending_provider_turn".to_string(),
        json!({
            "payload": provider_resume_payload(payload),
            "answer": carry.answer.clone(),
            "usage": carry.usage.clone(),
            "tool_calls": carry.tool_calls,
            "next_step": carry.next_step,
            "permission": permission_ruleset.as_str(),
            "skip_permissions": skip_permissions,
        }),
    );
}

fn take_pending_provider_turn(session: &mut Session) -> Option<RuntimeProviderResume> {
    let pending = session.metadata.remove("pending_provider_turn")?;
    let permission_raw = pending
        .get("permission")
        .and_then(Value::as_str)
        .unwrap_or("FULL");
    Some(RuntimeProviderResume {
        payload: pending.get("payload").cloned().unwrap_or_else(|| json!({})),
        carry: RuntimeProviderLoopCarry {
            answer: pending
                .get("answer")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            usage: usage_from_provider_json(pending.get("usage")),
            tool_calls: pending
                .get("tool_calls")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            next_step: pending
                .get("next_step")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .max(1),
        },
        permission_ruleset: parse_permission_ruleset(permission_raw).ok()?,
        skip_permissions: pending
            .get("skip_permissions")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn usage_from_provider_json(value: Option<&Value>) -> Usage {
    let input_tokens = value
        .and_then(|item| {
            item.get("input_tokens")
                .or_else(|| item.get("prompt_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or_default();
    let output_tokens = value
        .and_then(|item| {
            item.get("output_tokens")
                .or_else(|| item.get("completion_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or_default();
    Usage {
        input_tokens,
        output_tokens,
        cost: 0.0,
    }
}

fn usage_value_from_provider(
    usage: &Usage,
    tool_calls: u64,
    fallback_input: &str,
    fallback_output: &str,
) -> Value {
    let fallback = usage_payload(fallback_input, fallback_output, tool_calls);
    let input_tokens = if usage.input_tokens == 0 {
        fallback["input_tokens"].as_u64().unwrap_or_default()
    } else {
        usage.input_tokens
    };
    let output_tokens = if usage.output_tokens == 0 {
        fallback["output_tokens"].as_u64().unwrap_or_default()
    } else {
        usage.output_tokens
    };
    let tool_tokens = tool_calls.saturating_mul(16);
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "tool_tokens": tool_tokens,
        "total_tokens": input_tokens + output_tokens + tool_tokens,
        "tool_calls": tool_calls,
        "cost": usage.cost,
        "estimated": usage.input_tokens == 0 && usage.output_tokens == 0,
    })
}

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn runtime_chat_message(role: Role, content: String) -> ChatMessage {
    ChatMessage {
        role,
        content,
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::new(),
    }
}

fn assistant_message_for_provider_step(content: String, tool_calls: &[ToolCall]) -> ChatMessage {
    let mut message = runtime_chat_message(Role::Assistant, content);
    if !tool_calls.is_empty() {
        message.metadata.insert(
            "tool_calls".to_string(),
            Value::Array(tool_calls.iter().map(openai_tool_call_value).collect()),
        );
    }
    message
}

fn openai_tool_call_value(call: &ToolCall) -> Value {
    json!({
        "id": call.call_id.clone(),
        "call_id": call.call_id.clone(),
        "type": "function",
        "function": {
            "name": call.name.clone(),
            "arguments": python_json_dumps(&call.input),
        },
        "name": call.name.clone(),
        "input": call.input.clone(),
    })
}

fn run_provider_loop(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    payload: &Value,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
    mut events: Vec<Value>,
    mut carry: RuntimeProviderLoopCarry,
) -> Result<Value, String> {
    let max_steps = provider_max_steps(payload);
    let toolkit = Toolkit::with_builtins();
    let mut ctx = ToolContext::new(&session.directory)
        .with_session_id(session.id.clone())
        .with_permission_ruleset(permission_ruleset.clone())
        .with_dangerously_skip_permissions(skip_permissions);
    if let Some(answers) = payload
        .get("question_answers")
        .or_else(|| payload.get("answers"))
        .and_then(question_answers_from_json)
    {
        ctx.set_question_answers(answers);
    }

    let mut persisted_events = 0;
    append_unpersisted_app_events(
        &store.root,
        &session.id,
        run_id,
        &events,
        &mut persisted_events,
    );
    while carry.next_step <= max_steps {
        let step = carry.next_step;
        let mut streamed_text = false;
        let session_id = session.id.clone();
        let root = store.root.clone();
        let mut on_provider_stream = |event: &ProviderStreamEvent| {
            if let ProviderStreamEvent::TextDelta { text } = event
                && !text.is_empty()
            {
                streamed_text = true;
                events.push(json!({
                    "method": "item/agentMessage/delta",
                    "params": {
                        "thread_id": session_id.clone(),
                        "session_id": session_id.clone(),
                        "turn_id": run_id,
                        "run_id": run_id,
                        "step": step,
                        "event": {"id": format!("assistant_{step}"), "text": text.clone()},
                        "delta": text.clone(),
                    }
                }));
                append_unpersisted_app_events(
                    &root,
                    &session_id,
                    run_id,
                    &events,
                    &mut persisted_events,
                );
            }
        };
        let provider_result =
            provider_turn_result(session, payload, Some(&mut on_provider_stream))?;
        add_usage(&mut carry.usage, &provider_result.usage);
        if provider_result.source == "provider_missing_api_key" {
            events.push(json!({
                "method": "runtime/warning",
                "params": {
                    "session_id": session.id.clone(),
                    "turn_id": run_id,
                    "message": provider_result.answer.clone(),
                    "code": "provider_missing_api_key",
                }
            }));
        }
        if !provider_result.answer.is_empty() {
            carry.answer.push_str(&provider_result.answer);
            if !streamed_text {
                events.push(json!({
                    "method": "item/agentMessage/delta",
                    "params": {
                        "thread_id": session.id.clone(),
                        "session_id": session.id.clone(),
                        "turn_id": run_id,
                        "run_id": run_id,
                        "step": step,
                        "event": {"id": format!("assistant_{step}"), "text": provider_result.answer.clone()},
                        "delta": provider_result.answer.clone(),
                    }
                }));
            }
            let _ = store.append_part(
                &session.id,
                run_id,
                "text",
                SessionPartOptions {
                    attributes: BTreeMap::from([
                        ("role".to_string(), json!("assistant")),
                        (
                            "chars".to_string(),
                            json!(provider_result.answer.chars().count()),
                        ),
                    ]),
                    step_index: Some(step),
                    ..SessionPartOptions::default()
                },
            );
        }

        let assistant = assistant_message_for_provider_step(
            provider_result.answer.clone(),
            &provider_result.tool_calls,
        );
        let assistant_index = session.messages.len() as u64;
        session.add(assistant.clone());
        let _ = store.append_message(session, &assistant, run_id, assistant_index);

        if provider_result.tool_calls.is_empty() {
            return finish_provider_loop(
                store,
                session,
                run_id,
                events,
                &mut persisted_events,
                carry,
                &provider_result.finish_reason,
            );
        }

        let resume_carry = RuntimeProviderLoopCarry {
            next_step: step.saturating_add(1),
            ..carry.clone()
        };
        for tool_call in &provider_result.tool_calls {
            carry.tool_calls = carry.tool_calls.saturating_add(1);
            let pending_carry = RuntimeProviderLoopCarry {
                tool_calls: carry.tool_calls,
                next_step: step.saturating_add(1),
                ..resume_carry.clone()
            };
            if let Some(paused) = execute_provider_tool_call(
                store,
                session,
                run_id,
                payload,
                step,
                tool_call,
                &toolkit,
                &mut ctx,
                &permission_ruleset,
                skip_permissions,
                &pending_carry,
                &mut events,
                &mut persisted_events,
            )? {
                return Ok(paused);
            }
        }

        carry.next_step = step.saturating_add(1);
    }

    session.status = SessionStatus::Idle;
    let _ = store.finish_run(
        session,
        run_id,
        "failed",
        max_steps,
        Some("max_steps"),
        Some("agent loop exceeded max_steps"),
    );
    let usage = usage_value_from_provider(
        &carry.usage,
        carry.tool_calls,
        &latest_user_message(session),
        &carry.answer,
    );
    let trace = trace_payload(session, run_id, carry.tool_calls);
    events.push(json!({
        "method": "turn/failed",
        "params": {
            "session_id": session.id.clone(),
            "turn_id": run_id,
            "status": "failed",
            "error": "agent loop exceeded max_steps",
            "usage": usage,
            "trace": trace,
        }
    }));
    append_unpersisted_app_events(
        &store.root,
        &session.id,
        run_id,
        &events,
        &mut persisted_events,
    );
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "status": "failed",
        "events": events,
    }))
}

#[allow(clippy::too_many_arguments)]
fn execute_provider_tool_call(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    payload: &Value,
    step: u64,
    tool_call: &ToolCall,
    toolkit: &Toolkit,
    ctx: &mut ToolContext,
    permission_ruleset: &PermissionRuleset,
    skip_permissions: bool,
    pending_carry: &RuntimeProviderLoopCarry,
    events: &mut Vec<Value>,
    persisted_events: &mut usize,
) -> Result<Option<Value>, String> {
    events.push(json!({
        "method": "item/toolCall/started",
        "params": {
            "session_id": session.id.clone(),
            "turn_id": run_id,
            "run_id": run_id,
            "step": step,
            "call_id": tool_call.call_id.clone(),
            "name": tool_call.name.clone(),
            "input": tool_call.input.clone(),
        }
    }));
    append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
    let _ = store.record_event(
        &session.id,
        run_id,
        "tool.call.started",
        SessionEventOptions {
            kind: "tool".to_string(),
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("input".to_string(), tool_call.input.clone()),
                ("step".to_string(), json!(step)),
            ]),
            ..SessionEventOptions::default()
        },
    );

    if tool_call.name == "question" && ctx.question_answers.is_none() {
        let question = question_payload_for_tool_call(session, run_id, step, tool_call);
        session.status = SessionStatus::Paused;
        session
            .metadata
            .insert("pending_question".to_string(), question.clone());
        session.metadata.remove("pending_question_response");
        store_pending_provider_turn(
            session,
            payload,
            pending_carry,
            permission_ruleset.clone(),
            skip_permissions,
        );
        let _ = store.record_event(
            &session.id,
            run_id,
            "question.requested",
            SessionEventOptions {
                kind: "question".to_string(),
                attributes: BTreeMap::from([
                    ("call_id".to_string(), json!(tool_call.call_id.clone())),
                    (
                        "questions".to_string(),
                        tool_call
                            .input
                            .get("questions")
                            .cloned()
                            .unwrap_or_else(|| json!([])),
                    ),
                ]),
                ..SessionEventOptions::default()
            },
        );
        let _ = store.save_state(session, Some(run_id));
        events.push(json!({
            "method": "item/question/requested",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "status": "waiting_question",
                "event": question,
            }
        }));
        append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
        return Ok(Some(json!({
            "session_id": session.id,
            "turn_id": run_id,
            "status": "waiting_question",
            "events": events,
        })));
    }

    let change_before = capture_file_change_before(session, tool_call);
    let mut tool_result = toolkit.execute(
        &tool_call.name,
        tool_call.input.clone(),
        &tool_call.call_id,
        ctx,
    );
    if tool_result
        .metadata
        .get("requires_approval")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut approval =
            approval_payload_for_tool_call(session, run_id, step, tool_call, &tool_result.metadata);
        if let Some(preview) = change_before
            .as_ref()
            .and_then(|before| file_change_preview(before, tool_call))
            && let Some(object) = approval.as_object_mut()
        {
            object.insert("preview".to_string(), preview);
        }
        session.status = SessionStatus::Paused;
        session
            .metadata
            .insert("pending_approval".to_string(), approval.clone());
        session.metadata.remove("pending_approval_response");
        store_pending_provider_turn(
            session,
            payload,
            pending_carry,
            permission_ruleset.clone(),
            skip_permissions,
        );
        let _ = store.record_event(
            &session.id,
            run_id,
            "approval.requested",
            SessionEventOptions {
                kind: "approval".to_string(),
                attributes: BTreeMap::from([
                    ("call_id".to_string(), json!(tool_call.call_id.clone())),
                    ("name".to_string(), json!(tool_call.name.clone())),
                    ("approval".to_string(), approval.clone()),
                ]),
                ..SessionEventOptions::default()
            },
        );
        let _ = store.save_state(session, Some(run_id));
        events.push(json!({
            "method": "turn/approval_requested",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "status": "waiting_approval",
                "approval": approval,
            }
        }));
        append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
        return Ok(Some(json!({
            "session_id": session.id,
            "turn_id": run_id,
            "status": "waiting_approval",
            "events": events,
        })));
    }

    append_completed_tool_result(
        store,
        session,
        run_id,
        step,
        tool_call,
        change_before,
        &mut tool_result,
        events,
    )?;
    append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn append_completed_tool_result(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    step: u64,
    tool_call: &ToolCall,
    change_before: Option<FileChangeBefore>,
    tool_result: &mut ToolResult,
    events: &mut Vec<Value>,
) -> Result<(), String> {
    let failed = tool_result.error.is_some();
    let patch = complete_file_change(
        store,
        session,
        run_id,
        tool_call,
        change_before,
        tool_result,
    );
    if let Some(change) = patch.as_ref() {
        tool_result
            .metadata
            .insert("patch".to_string(), public_file_change(change));
        tool_result.metadata.insert(
            "patch_id".to_string(),
            change.get("id").cloned().unwrap_or(Value::Null),
        );
        tool_result.metadata.insert(
            "diff".to_string(),
            change.get("diff").cloned().unwrap_or(Value::Null),
        );
    }
    events.push(json!({
        "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
        "params": {
            "session_id": session.id.clone(),
            "turn_id": run_id,
            "run_id": run_id,
            "step": step,
            "call_id": tool_call.call_id.clone(),
            "name": tool_call.name.clone(),
            "output": tool_result.output.clone(),
            "error": tool_result.error.clone(),
            "metadata": tool_result.metadata.clone(),
        }
    }));
    if let Some(change) = patch.as_ref() {
        events.push(patch_detected_event(session, run_id, change));
    }
    append_tool_result_to_session(store, session, run_id, step, tool_call, tool_result)
}

fn append_tool_result_to_session(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    step: u64,
    tool_call: &ToolCall,
    tool_result: &ToolResult,
) -> Result<(), String> {
    let failed = tool_result.error.is_some();
    let _ = store.record_event(
        &session.id,
        run_id,
        if failed {
            "tool.call.failed"
        } else {
            "tool.call.finished"
        },
        SessionEventOptions {
            kind: "tool".to_string(),
            status: if failed {
                "error".to_string()
            } else {
                "ok".to_string()
            },
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("error".to_string(), json!(tool_result.error.clone())),
                ("metadata".to_string(), json!(tool_result.metadata.clone())),
                ("step".to_string(), json!(step)),
            ]),
            ..SessionEventOptions::default()
        },
    );
    let _ = store.append_part(
        &session.id,
        run_id,
        "tool_result",
        SessionPartOptions {
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("failed".to_string(), json!(failed)),
            ]),
            step_index: Some(step),
            ..SessionPartOptions::default()
        },
    );
    let mut tool_message = runtime_chat_message(
        Role::Tool,
        tool_result.error.as_ref().map_or_else(
            || tool_result.output.clone(),
            |error| format!("Tool failed: {error}"),
        ),
    );
    tool_message.name = Some(tool_call.name.clone());
    tool_message.tool_call_id = Some(tool_call.call_id.clone());
    tool_message
        .metadata
        .insert("tool_result".to_string(), json!(tool_result));
    let tool_index = session.messages.len() as u64;
    session.add(tool_message.clone());
    store
        .append_message(session, &tool_message, run_id, tool_index)
        .map_err(|error| format!("failed to record tool message: {error}"))
}

fn finish_provider_loop(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    mut events: Vec<Value>,
    persisted_events: &mut usize,
    carry: RuntimeProviderLoopCarry,
    finish_reason: &str,
) -> Result<Value, String> {
    session.status = SessionStatus::Idle;
    session.metadata.remove("pending_provider_turn");
    let steps = carry.next_step.max(1);
    let _ = store.finish_run(
        session,
        run_id,
        "completed",
        steps,
        Some(finish_reason),
        None,
    );
    let usage = usage_value_from_provider(
        &carry.usage,
        carry.tool_calls,
        &latest_user_message(session),
        &carry.answer,
    );
    let trace = trace_payload(session, run_id, carry.tool_calls);
    record_usage_event(store, session, run_id, &usage);
    events.push(json!({
        "method": "turn/completed",
        "params": {
            "thread_id": session.id.clone(),
            "session_id": session.id.clone(),
            "turn_id": run_id,
            "status": "completed",
            "final_answer": carry.answer,
            "usage": usage,
            "trace": trace,
            "finish_reason": finish_reason,
        }
    }));
    append_unpersisted_app_events(&store.root, &session.id, run_id, &events, persisted_events);
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "status": "completed",
        "turn": {
            "id": run_id,
            "session_id": session.id,
            "status": "completed",
            "final_answer": events.last().and_then(|event| event.get("params")).and_then(|params| params.get("final_answer")).cloned().unwrap_or_else(|| json!("")),
            "agent": session_text_metadata(session, "agent", "server"),
            "model": session_text_metadata(session, "model", &default_model_id()),
            "variant": session_text_metadata(session, "variant", "default"),
            "thinking": session_text_metadata(session, "thinking", "medium"),
            "usage": usage,
            "trace": trace,
        },
        "events": events
    }))
}

fn start_turn_payload(
    config: &HttpRuntimeConfig,
    session_id: &str,
    body: &str,
) -> Result<Value, String> {
    let payload: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let input = payload
        .get("input")
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if input.trim().is_empty() {
        return Err("turn input is required".to_string());
    }
    let permission_ruleset = permission_ruleset_for_turn(&payload)?;
    let skip_permissions = skip_permissions_for_turn(&payload);
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .unwrap_or_else(|_| Session::new(session_id.to_string(), workspace(config)));
    let runtime_profile = apply_turn_runtime_profile(&mut session, &payload);
    let run_id = new_id("turn");
    session.status = SessionStatus::Running;
    let _ = store.start_run(
        &mut session,
        StartRunOptions {
            run_id: run_id.clone(),
            trace_id: new_id("trace"),
            agent_name: runtime_profile.agent.clone(),
            model_id: Some(runtime_profile.model.clone()),
            provider_id: Some("openagent".to_string()),
            permission: if skip_permissions {
                format!("auto_allow:{}", permission_ruleset.as_str())
            } else {
                permission_ruleset.as_str().to_string()
            },
            max_steps: 1,
            started_at_ms: None,
        },
    );
    let user = ChatMessage {
        role: Role::User,
        content: input.to_string(),
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::new(),
    };
    let user_index = session.messages.len() as u64;
    session.add(user.clone());
    let _ = store.append_message(&session, &user, &run_id, user_index);
    let tool_calls = tool_calls_from_turn_payload(&payload)?;
    if !tool_calls.is_empty() {
        return run_http_tool_turn(
            &store,
            &mut session,
            &run_id,
            tool_calls,
            permission_ruleset,
            skip_permissions,
        );
    }
    let _ = runtime_profile;
    let initial_events = vec![turn_started_event(&session, &run_id)];
    run_provider_loop(
        &store,
        &mut session,
        &run_id,
        &payload,
        permission_ruleset,
        skip_permissions,
        initial_events,
        RuntimeProviderLoopCarry::default(),
    )
}

fn run_http_tool_turn(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    tool_calls: Vec<ToolCall>,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
) -> Result<Value, String> {
    let toolkit = Toolkit::with_builtins();
    let tool_call_count = tool_calls.len() as u64;
    let mut ctx = ToolContext::new(&session.directory)
        .with_session_id(session.id.clone())
        .with_permission_ruleset(permission_ruleset)
        .with_dangerously_skip_permissions(skip_permissions);
    let mut events = vec![turn_started_event(session, run_id)];

    for (index, tool_call) in tool_calls.into_iter().enumerate() {
        let step = index as u64 + 1;
        events.push(json!({
            "method": "item/toolCall/started",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "call_id": tool_call.call_id.clone(),
                "name": tool_call.name.clone(),
                "input": tool_call.input.clone(),
            }
        }));
        let change_before = capture_file_change_before(session, &tool_call);
        let mut tool_result = toolkit.execute(
            &tool_call.name,
            tool_call.input.clone(),
            &tool_call.call_id,
            &mut ctx,
        );
        if tool_result
            .metadata
            .get("requires_approval")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let mut approval = approval_payload_for_tool_call(
                session,
                run_id,
                step,
                &tool_call,
                &tool_result.metadata,
            );
            if let Some(preview) = change_before
                .as_ref()
                .and_then(|before| file_change_preview(before, &tool_call))
                && let Some(object) = approval.as_object_mut()
            {
                object.insert("preview".to_string(), preview);
            }
            session.status = SessionStatus::Paused;
            session
                .metadata
                .insert("pending_approval".to_string(), approval.clone());
            let _ = store.record_event(
                &session.id,
                run_id,
                "approval.requested",
                SessionEventOptions {
                    kind: "approval".to_string(),
                    attributes: BTreeMap::from([
                        ("call_id".to_string(), json!(tool_call.call_id)),
                        ("name".to_string(), json!(tool_call.name)),
                        ("approval".to_string(), approval.clone()),
                    ]),
                    ..SessionEventOptions::default()
                },
            );
            let _ = store.save_state(session, Some(run_id));
            events.push(json!({
                "method": "turn/approval_requested",
                "params": {
                    "session_id": session.id.clone(),
                    "turn_id": run_id,
                    "status": "waiting_approval",
                    "approval": approval,
                }
            }));
            append_app_events(&store.root, &session.id, run_id, &events);
            return Ok(json!({
                "session_id": session.id,
                "turn_id": run_id,
                "status": "waiting_approval",
                "events": events,
            }));
        }

        let failed = tool_result.error.is_some();
        let patch = complete_file_change(
            store,
            session,
            run_id,
            &tool_call,
            change_before,
            &tool_result,
        );
        if let Some(change) = patch.as_ref() {
            tool_result
                .metadata
                .insert("patch".to_string(), public_file_change(change));
            tool_result.metadata.insert(
                "patch_id".to_string(),
                change.get("id").cloned().unwrap_or(Value::Null),
            );
            tool_result.metadata.insert(
                "diff".to_string(),
                change.get("diff").cloned().unwrap_or(Value::Null),
            );
        }
        events.push(json!({
            "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "call_id": tool_call.call_id.clone(),
                "name": tool_call.name.clone(),
                "output": tool_result.output,
                "error": tool_result.error,
                "metadata": tool_result.metadata,
            }
        }));
        if let Some(change) = patch.as_ref() {
            events.push(patch_detected_event(session, run_id, change));
        }
    }

    let answer = if tool_calls_completed_successfully(&events) {
        "tool execution completed".to_string()
    } else {
        "tool execution failed".to_string()
    };
    let assistant = ChatMessage {
        role: Role::Assistant,
        content: answer.clone(),
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::new(),
    };
    let assistant_index = session.messages.len() as u64;
    session.add(assistant.clone());
    session.status = SessionStatus::Idle;
    let _ = store.append_message(session, &assistant, run_id, assistant_index);
    let _ = store.finish_run(session, run_id, "completed", 1, Some("stop"), None);
    let input = latest_user_message(session);
    let usage = usage_payload(&input, &answer, tool_call_count);
    let trace = trace_payload(session, run_id, tool_call_count);
    record_usage_event(store, session, run_id, &usage);
    events.push(json!({
        "method": "turn/completed",
        "params": {
            "thread_id": session.id.clone(),
            "turn_id": run_id,
            "status": "completed",
            "final_answer": answer,
            "usage": usage,
            "trace": trace,
        }
    }));
    append_app_events(&store.root, &session.id, run_id, &events);
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "status": "completed",
        "events": events,
    }))
}

fn respond_approval_payload(
    config: &HttpRuntimeConfig,
    path: &str,
    body: &str,
) -> Result<Value, String> {
    let (turn_id, request_id) = parse_turn_approval_path(path)?;
    let payload: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let response = approval_response_payload(&payload)?;
    let action = response
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let store = FileSessionStore::new(session_root(config));
    let mut session = find_session_with_pending_approval(&store, &turn_id, &request_id)?;
    let approval = session
        .metadata
        .get("pending_approval")
        .cloned()
        .ok_or_else(|| "pending approval not found".to_string())?;
    let run_id = approval
        .get("run_id")
        .or_else(|| approval.get("turn_id"))
        .and_then(Value::as_str)
        .unwrap_or(&turn_id)
        .to_string();
    let mut resolved = approval.clone();
    if let Some(object) = resolved.as_object_mut() {
        object.insert("action".to_string(), json!(action));
        object.insert("resolved_at_ms".to_string(), json!(now_ms()));
        if let Some(scope) = response.get("scope") {
            object.insert("scope".to_string(), scope.clone());
        }
        if let Some(note) = response.get("note") {
            object.insert("note".to_string(), note.clone());
        }
    }
    session.metadata.remove("pending_approval");

    let mut events = vec![json!({
        "method": "turn/approval_resolved",
        "params": {
            "session_id": session.id.clone(),
            "thread_id": session.id.clone(),
            "turn_id": run_id.clone(),
            "request_id": request_id.clone(),
            "status": if action == "allow" { "running" } else { "denied" },
            "approval": resolved,
        }
    })];
    let _ = store.record_event(
        &session.id,
        &run_id,
        "approval.resolved",
        SessionEventOptions {
            kind: "approval".to_string(),
            status: action.to_string(),
            attributes: BTreeMap::from([
                ("request_id".to_string(), json!(request_id)),
                ("action".to_string(), json!(action)),
            ]),
            ..SessionEventOptions::default()
        },
    );

    if action == "allow" {
        let tool_call = pending_approval_tool_call(&approval)?;
        let toolkit = Toolkit::with_builtins();
        let mut ctx = ToolContext::new(&session.directory)
            .with_session_id(session.id.clone())
            .with_dangerously_skip_permissions(true);
        let change_before = capture_file_change_before(&session, &tool_call);
        let mut tool_result = toolkit.execute(
            &tool_call.name,
            tool_call.input.clone(),
            &tool_call.call_id,
            &mut ctx,
        );
        append_completed_tool_result(
            &store,
            &mut session,
            &run_id,
            approval.get("step").and_then(Value::as_u64).unwrap_or(1),
            &tool_call,
            change_before,
            &mut tool_result,
            &mut events,
        )?;
        if let Some(resume) = take_pending_provider_turn(&mut session) {
            session.status = SessionStatus::Running;
            return run_provider_loop(
                &store,
                &mut session,
                &run_id,
                &resume.payload,
                resume.permission_ruleset,
                resume.skip_permissions,
                events,
                resume.carry,
            );
        }
        let failed = tool_result.error.is_some();
        let answer = if failed {
            "approval resolved, but tool execution failed".to_string()
        } else {
            "approval resolved".to_string()
        };
        let input = latest_user_message(&session);
        let usage = usage_payload(&input, &answer, 1);
        let trace = trace_payload(&session, &run_id, 1);
        record_usage_event(&store, &session, &run_id, &usage);
        session.status = SessionStatus::Idle;
        let _ = store.finish_run(
            &session,
            &run_id,
            if failed { "failed" } else { "completed" },
            1,
            Some(if failed { "tool_error" } else { "stop" }),
            None,
        );
        events.push(json!({
            "method": "turn/completed",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "status": if failed { "failed" } else { "completed" },
                "final_answer": answer,
                "usage": usage,
                "trace": trace,
            }
        }));
    } else {
        session.metadata.remove("pending_provider_turn");
        session.status = SessionStatus::Idle;
        let _ = store.finish_run(
            &session,
            &run_id,
            "failed",
            1,
            Some("permission_denied"),
            Some("approval denied"),
        );
        events.push(json!({
            "method": "turn/failed",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id.clone(),
                "status": "failed",
                "error": "approval denied",
            }
        }));
    }
    let _ = store.save_state(&session, Some(&run_id));
    append_app_events(&store.root, &session.id, &run_id, &events);
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "approval": response,
        "events": events,
    }))
}

fn respond_question_payload(
    config: &HttpRuntimeConfig,
    path: &str,
    body: &str,
) -> Result<Value, String> {
    let (turn_id, request_id) = parse_turn_question_reply_path(path)?;
    let payload: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let response = if payload
        .get("dismissed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        question_dismiss_payload(&payload)
    } else {
        question_reply_payload(&payload)?
    };
    let store = FileSessionStore::new(session_root(config));
    let mut session = find_session_with_pending_question(&store, &turn_id, &request_id)?;
    let question = session
        .metadata
        .get("pending_question")
        .cloned()
        .ok_or_else(|| "pending question not found".to_string())?;
    let run_id = question
        .get("run_id")
        .or_else(|| question.get("turn_id"))
        .and_then(Value::as_str)
        .unwrap_or(&turn_id)
        .to_string();
    session.metadata.remove("pending_question");

    let mut events = vec![json!({
        "method": "item/question/resolved",
        "params": {
            "session_id": session.id.clone(),
            "thread_id": session.id.clone(),
            "turn_id": run_id.clone(),
            "request_id": request_id.clone(),
            "status": if response.get("dismissed").and_then(Value::as_bool).unwrap_or(false) {
                "dismissed"
            } else {
                "answered"
            },
            "question": response.clone(),
        }
    })];

    if response
        .get("dismissed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        session.metadata.remove("pending_provider_turn");
        session.status = SessionStatus::Idle;
        let _ = store.finish_run(
            &session,
            &run_id,
            "failed",
            1,
            Some("question_dismissed"),
            Some("question dismissed"),
        );
        events.push(json!({
            "method": "turn/failed",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id.clone(),
                "status": "failed",
                "error": response.get("note").and_then(Value::as_str).unwrap_or("question dismissed"),
            }
        }));
        let _ = store.save_state(&session, Some(&run_id));
        append_app_events(&store.root, &session.id, &run_id, &events);
        return Ok(json!({
            "session_id": session.id,
            "turn_id": run_id,
            "request_id": request_id,
            "question": response,
            "events": events,
        }));
    }

    let tool_call = pending_question_tool_call(&question)?;
    let mut ctx = ToolContext::new(&session.directory).with_session_id(session.id.clone());
    let answers = response
        .get("answers")
        .and_then(question_answers_from_json)
        .unwrap_or_default();
    ctx.set_question_answers(answers);
    let toolkit = Toolkit::with_builtins();
    let mut tool_result = toolkit.execute(
        "question",
        tool_call.input.clone(),
        &tool_call.call_id,
        &mut ctx,
    );
    append_completed_tool_result(
        &store,
        &mut session,
        &run_id,
        question.get("step").and_then(Value::as_u64).unwrap_or(1),
        &tool_call,
        None,
        &mut tool_result,
        &mut events,
    )?;

    if let Some(resume) = take_pending_provider_turn(&mut session) {
        session.status = SessionStatus::Running;
        return run_provider_loop(
            &store,
            &mut session,
            &run_id,
            &resume.payload,
            resume.permission_ruleset,
            resume.skip_permissions,
            events,
            resume.carry,
        );
    }
    session.status = SessionStatus::Idle;
    let answer = "question answered".to_string();
    let input = latest_user_message(&session);
    let usage = usage_payload(&input, &answer, 1);
    let trace = trace_payload(&session, &run_id, 1);
    record_usage_event(&store, &session, &run_id, &usage);
    let _ = store.finish_run(&session, &run_id, "completed", 1, Some("stop"), None);
    let _ = store.save_state(&session, Some(&run_id));
    events.push(json!({
        "method": "turn/completed",
        "params": {
            "session_id": session.id.clone(),
            "turn_id": run_id.clone(),
            "status": "completed",
            "final_answer": answer,
            "usage": usage,
            "trace": trace,
        }
    }));
    append_app_events(&store.root, &session.id, &run_id, &events);
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "request_id": request_id,
        "question": response,
        "events": events,
    }))
}

fn interrupt_turn_payload(config: &HttpRuntimeConfig, turn_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let (session_id, mut session) = find_session_for_turn(&store, turn_id)?;
    session.status = SessionStatus::Stop;
    let _ = store.finish_run(
        &session,
        turn_id,
        "failed",
        1,
        Some("interrupted"),
        Some("interrupt requested"),
    );
    let event = json!({
        "method": "turn/interrupted",
        "params": {
            "session_id": session_id,
            "thread_id": session_id,
            "turn_id": turn_id,
            "status": "interrupted",
            "error": "interrupt requested",
        }
    });
    append_app_events(
        &store.root,
        &session_id,
        turn_id,
        std::slice::from_ref(&event),
    );
    Ok(json!({
        "session_id": session_id,
        "turn_id": turn_id,
        "status": "interrupted",
        "events": [event],
    }))
}

fn enqueue_tui_control_payload(
    config: &HttpRuntimeConfig,
    path: &str,
    body: &str,
) -> Result<Value, String> {
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let request = tui_control_request_for_path(path, &payload)?;
    let mut queue = read_json_array(&tui_control_queue_path(config));
    queue.push(request.to_value());
    write_json_value(&tui_control_queue_path(config), &Value::Array(queue))?;
    Ok(json!({"queued": true, "request": request.to_value()}))
}

fn pop_tui_control_payload(config: &HttpRuntimeConfig) -> Value {
    let path = tui_control_queue_path(config);
    let mut queue = read_json_array(&path);
    if queue.is_empty() {
        return control_next_payload(None);
    }
    let next = queue.remove(0);
    let _ = write_json_value(&path, &Value::Array(queue));
    let request = next.as_object().map(|_| {
        openagent_app_server::TuiControlRequest::new(
            next.get("path").and_then(Value::as_str).unwrap_or_default(),
            next.get("body").cloned().unwrap_or(Value::Null),
        )
    });
    control_next_payload(request.as_ref())
}

fn record_tui_control_response(config: &HttpRuntimeConfig, body: &str) -> Value {
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let response = record_control_response_payload(payload);
    append_json_line(&tui_control_responses_path(config), &response);
    response
}

fn find_session_with_pending_approval(
    store: &FileSessionStore,
    turn_id: &str,
    request_id: &str,
) -> Result<Session, String> {
    for entry in fs::read_dir(&store.root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.path().is_dir() {
            continue;
        }
        let state = read_json_file(&entry.path().join("state.latest.json"));
        let Some(pending) = state
            .get("metadata")
            .and_then(|metadata| metadata.get("pending_approval"))
        else {
            continue;
        };
        let same_turn = pending
            .get("turn_id")
            .or_else(|| pending.get("run_id"))
            .and_then(Value::as_str)
            == Some(turn_id);
        let same_request = pending.get("request_id").and_then(Value::as_str) == Some(request_id);
        if same_turn && same_request {
            let session_id = state
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "pending approval session is missing session_id".to_string())?;
            return store
                .load_session(session_id)
                .map_err(|error| error.to_string());
        }
    }
    Err("pending approval not found".to_string())
}

fn find_session_with_pending_question(
    store: &FileSessionStore,
    turn_id: &str,
    request_id: &str,
) -> Result<Session, String> {
    for entry in fs::read_dir(&store.root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.path().is_dir() {
            continue;
        }
        let state = read_json_file(&entry.path().join("state.latest.json"));
        let Some(pending) = state
            .get("metadata")
            .and_then(|metadata| metadata.get("pending_question"))
        else {
            continue;
        };
        let same_turn = pending
            .get("turn_id")
            .or_else(|| pending.get("run_id"))
            .and_then(Value::as_str)
            == Some(turn_id);
        let same_request = pending.get("request_id").and_then(Value::as_str) == Some(request_id);
        if same_turn && same_request {
            let session_id = state
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "pending question session is missing session_id".to_string())?;
            return store
                .load_session(session_id)
                .map_err(|error| error.to_string());
        }
    }
    Err("pending question not found".to_string())
}

fn pending_approval_tool_call(approval: &Value) -> Result<ToolCall, String> {
    let name = approval
        .get("tool_name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "pending approval missing tool_name".to_string())?;
    let call_id = approval
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("call_approval");
    Ok(ToolCall {
        name: name.to_string(),
        input: approval
            .get("tool_input")
            .cloned()
            .unwrap_or_else(|| json!({})),
        call_id: call_id.to_string(),
    })
}

fn pending_question_tool_call(question: &Value) -> Result<ToolCall, String> {
    let name = question
        .get("tool_name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("question");
    let call_id = question
        .get("call_id")
        .or_else(|| question.get("tool_call_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("call_question");
    Ok(ToolCall {
        name: name.to_string(),
        input: question
            .get("tool_input")
            .cloned()
            .unwrap_or_else(|| json!({})),
        call_id: call_id.to_string(),
    })
}

fn approval_payload_for_tool_call(
    session: &Session,
    run_id: &str,
    step: u64,
    call: &ToolCall,
    metadata: &BTreeMap<String, Value>,
) -> Value {
    json!({
        "request_id": format!("approval_{}", call.call_id),
        "session_id": session.id,
        "turn_id": run_id,
        "run_id": run_id,
        "step": step,
        "tool_name": call.name,
        "tool_input": call.input,
        "call_id": call.call_id,
        "created_at_ms": now_ms(),
        "permission_action": metadata.get("permission_action").cloned().unwrap_or_else(|| json!("ask")),
        "permission_pattern": metadata.get("permission_pattern").cloned().unwrap_or_else(|| json!("")),
        "reason": metadata.get("error_kind").cloned().unwrap_or_else(|| json!("permission_required")),
        "metadata": metadata,
    })
}

fn question_payload_for_tool_call(
    session: &Session,
    run_id: &str,
    step: u64,
    call: &ToolCall,
) -> Value {
    json!({
        "request_id": format!("question_{}", call.call_id),
        "session_id": session.id,
        "turn_id": run_id,
        "run_id": run_id,
        "step": step,
        "tool_name": call.name,
        "tool_input": call.input,
        "tool_call_id": call.call_id,
        "call_id": call.call_id,
        "questions": call.input.get("questions").cloned().unwrap_or_else(|| json!([])),
        "created_at_ms": now_ms(),
    })
}

fn question_answers_from_json(value: &Value) -> Option<Vec<Vec<String>>> {
    let items = value.as_array()?;
    if items.iter().all(Value::is_array) {
        return Some(
            items
                .iter()
                .map(|item| {
                    item.as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(value_to_answer_string)
                        .collect::<Vec<_>>()
                })
                .collect(),
        );
    }
    Some(
        items
            .iter()
            .filter_map(value_to_answer_string)
            .map(|answer| vec![answer])
            .collect(),
    )
}

fn value_to_answer_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_bool().map(|item| item.to_string()))
        .or_else(|| value.as_i64().map(|item| item.to_string()))
        .or_else(|| value.as_u64().map(|item| item.to_string()))
        .or_else(|| value.as_f64().map(|item| item.to_string()))
}

fn tool_calls_from_turn_payload(payload: &Value) -> Result<Vec<ToolCall>, String> {
    if let Some(tool_call) = payload.get("tool_call") {
        return Ok(vec![tool_call_from_value(tool_call, 0)?]);
    }
    if let Some(items) = payload.get("tool_calls").and_then(Value::as_array) {
        return items
            .iter()
            .enumerate()
            .map(|(index, item)| tool_call_from_value(item, index))
            .collect();
    }
    Ok(Vec::new())
}

fn tool_call_from_value(value: &Value, index: usize) -> Result<ToolCall, String> {
    let name = value
        .get("name")
        .or_else(|| value.get("tool"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "tool call name is required".to_string())?;
    Ok(ToolCall {
        name: name.to_string(),
        input: value
            .get("input")
            .or_else(|| value.get("arguments"))
            .cloned()
            .unwrap_or_else(|| json!({})),
        call_id: value
            .get("call_id")
            .or_else(|| value.get("id"))
            .and_then(Value::as_str)
            .map_or_else(|| format!("call_{index}"), str::to_string),
    })
}

fn permission_ruleset_for_turn(payload: &Value) -> Result<PermissionRuleset, String> {
    let raw = payload
        .get("permission")
        .or_else(|| payload.get("permissions"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| std::env::var("OPENAGENT_APP_PERMISSION").ok())
        .unwrap_or_else(|| "FULL".to_string());
    parse_permission_ruleset(&raw)
}

fn parse_permission_ruleset(raw: &str) -> Result<PermissionRuleset, String> {
    match raw.trim().to_ascii_uppercase().replace('-', "_").as_str() {
        "FULL" | "ALLOW" | "AUTO" => Ok(PermissionRuleset::Full),
        "READONLY" | "READ_ONLY" => Ok(PermissionRuleset::Readonly),
        "PLAN_ONLY" | "ASK" => Ok(PermissionRuleset::PlanOnly),
        "NONE" | "DENY" => Ok(PermissionRuleset::None),
        _ => Err("permission must be FULL, READONLY, PLAN_ONLY, or NONE".to_string()),
    }
}

fn skip_permissions_for_turn(payload: &Value) -> bool {
    payload
        .get("dangerously_skip_permissions")
        .or_else(|| payload.get("skip_permissions"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            std::env::var("OPENAGENT_APP_DANGEROUSLY_SKIP_PERMISSIONS")
                .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes"))
        })
}

fn provider_streaming_enabled_for_turn(payload: &Value) -> bool {
    payload
        .get("stream")
        .or_else(|| payload.get("provider_stream"))
        .or_else(|| payload.get("stream_provider"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            std::env::var("OPENAGENT_PROVIDER_STREAM")
                .map(|value| !matches!(value.as_str(), "0" | "false" | "FALSE" | "no"))
                .unwrap_or(true)
        })
}

fn append_app_events(root: &Path, session_id: &str, turn_id: &str, events: &[Value]) {
    let path = app_events_path(root, session_id, turn_id);
    let existing = read_jsonl_values(&path).len() as u64;
    for (index, event) in events.iter().enumerate() {
        let mut normalized = event.clone();
        if let Some(object) = normalized.as_object_mut() {
            object
                .entry("sequence".to_string())
                .or_insert_with(|| json!(existing + index as u64 + 1));
            object
                .entry("created_at_ms".to_string())
                .or_insert_with(|| json!(now_ms()));
            object
                .entry("global_sequence".to_string())
                .or_insert_with(|| json!(existing + index as u64 + 1));
        }
        append_json_line(&path, &normalized);
    }
}

fn append_unpersisted_app_events(
    root: &Path,
    session_id: &str,
    turn_id: &str,
    events: &[Value],
    persisted_events: &mut usize,
) {
    if *persisted_events >= events.len() {
        return;
    }
    append_app_events(root, session_id, turn_id, &events[*persisted_events..]);
    *persisted_events = events.len();
}

fn global_sse_frames(config: &HttpRuntimeConfig, request_path: &str) -> String {
    let last_id = last_event_id_from_path(request_path);
    let mut frames = String::new();
    for (index, event) in all_app_events(config).into_iter().enumerate() {
        let id = event
            .get("global_sequence")
            .or_else(|| event.get("sequence"))
            .and_then(Value::as_u64)
            .unwrap_or(index as u64 + 1);
        if id <= last_id {
            continue;
        }
        frames.push_str(&sse_frame(id, &event));
    }
    if frames.is_empty() {
        frames.push_str(": ping\n\n");
    }
    frames
}

fn turn_sse_frames(config: &HttpRuntimeConfig, turn_id: &str, request_path: &str) -> String {
    let last_id = last_event_id_from_path(request_path);
    let mut frames = String::new();
    for event in turn_app_events(config, turn_id) {
        let id = event
            .get("sequence")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if id <= last_id {
            continue;
        }
        frames.push_str(&sse_frame(id, &event));
    }
    if frames.is_empty() {
        frames.push_str(": ping\n\n");
    }
    frames
}

fn sse_frame(id: u64, event: &Value) -> String {
    let event_name = event
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("message");
    format!(
        "id: {id}\nevent: {event_name}\ndata: {}\n\n",
        python_json_dumps(event)
    )
}

fn all_app_events(config: &HttpRuntimeConfig) -> Vec<Value> {
    let root = session_root(config);
    let mut events = Vec::new();
    if let Ok(sessions) = fs::read_dir(&root) {
        for session in sessions.flatten() {
            let runs_dir = session.path().join("runs");
            if let Ok(runs) = fs::read_dir(runs_dir) {
                for run in runs.flatten() {
                    events.extend(read_jsonl_values(&run.path().join(APP_EVENTS_FILE)));
                }
            }
        }
    }
    events.sort_by_key(|event| {
        event
            .get("created_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    });
    for (index, event) in events.iter_mut().enumerate() {
        if let Some(object) = event.as_object_mut() {
            object.insert("global_sequence".to_string(), json!(index as u64 + 1));
        }
    }
    events
}

fn turn_app_events(config: &HttpRuntimeConfig, turn_id: &str) -> Vec<Value> {
    let root = session_root(config);
    if let Ok(sessions) = fs::read_dir(&root) {
        for session in sessions.flatten() {
            let path = app_events_path(&root, &session.file_name().to_string_lossy(), turn_id);
            if path.exists() {
                return read_jsonl_values(&path);
            }
        }
    }
    Vec::new()
}

fn app_events_path(root: &Path, session_id: &str, turn_id: &str) -> PathBuf {
    root.join(session_id)
        .join("runs")
        .join(turn_id)
        .join(APP_EVENTS_FILE)
}

fn last_event_id_from_path(path: &str) -> u64 {
    query_value(path, "last_event_id")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}

fn query_value(path: &str, name: &str) -> Option<String> {
    path.split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.replace("%20", " ").replace('+', " ")))
}

fn find_session_for_turn(
    store: &FileSessionStore,
    turn_id: &str,
) -> Result<(String, Session), String> {
    for entry in fs::read_dir(&store.root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.path().join("runs").join(turn_id).is_dir() {
            continue;
        }
        let session_id = entry.file_name().to_string_lossy().to_string();
        let session = store
            .load_session(&session_id)
            .map_err(|error| error.to_string())?;
        return Ok((session_id, session));
    }
    Err("turn not found".to_string())
}

fn tui_control_queue_path(config: &HttpRuntimeConfig) -> PathBuf {
    session_root(config).join(TUI_CONTROL_QUEUE_FILE)
}

fn tui_control_responses_path(config: &HttpRuntimeConfig) -> PathBuf {
    session_root(config).join(TUI_CONTROL_RESPONSES_FILE)
}

fn read_json_array(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
}

fn read_jsonl_values(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .ok()
        .map(|raw| {
            raw.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn write_json_value(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, python_json_dumps(value)).map_err(|error| error.to_string())
}

fn append_json_line(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{}", python_json_dumps(value));
    }
}

fn session_root(config: &HttpRuntimeConfig) -> PathBuf {
    config
        .session_store_root
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace(config).join(".openagent/sessions"))
}

fn workspace(config: &HttpRuntimeConfig) -> PathBuf {
    config
        .workspace
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn read_json_file(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn session_status_text(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::Paused => "paused",
        SessionStatus::Stop => "stop",
        SessionStatus::Compacting => "compacting",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}_{}", now_ms(), std::process::id())
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
        ..HttpRuntimeConfig::default()
    };
    let events = fixture_events();
    let text = emit_app_bridge_events(&events, "text", true);
    let emitted_json = emit_app_bridge_events(&events, "json", false);
    let sse_lines = [
        ": ping\n",
        "\n",
        "id: 1\n",
        "event: item/agentMessage/delta\n",
        "data: {\"sequence\": 1, \"method\": \"item/agentMessage/delta\", \"params\": {\"event\": {\"text\": \"provider fixture answer\"}}}\n",
        "\n",
        "id: 2\n",
        "event: turn/completed\n",
        "data: {\"sequence\": 2, \"method\": \"turn/completed\", \"params\": {\"status\": \"completed\", \"final_answer\": \"provider fixture answer\"}}\n",
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
            "params": {"event": {"text": "provider fixture answer"}},
        }),
        json!({
            "sequence": 2,
            "method": "turn/completed",
            "params": {"status": "completed", "final_answer": "provider fixture answer"},
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

    #[test]
    fn app_bridge_permission_approval_round_trip_executes_allowed_tool() {
        let root = std::env::temp_dir().join(format!("openagent-http-permission-{}", now_ms()));
        let workspace = root.join("workspace");
        let session_root = root.join("sessions");
        fs::create_dir_all(&workspace).expect("workspace");
        let config = HttpRuntimeConfig {
            serve_static: false,
            workspace: Some(workspace.to_string_lossy().to_string()),
            session_store_root: Some(session_root.to_string_lossy().to_string()),
            ..HttpRuntimeConfig::default()
        };
        let created = create_session_payload(
            &config,
            &python_json_dumps(&json!({"cwd": workspace.to_string_lossy()})),
        );
        let session_id = created
            .get("session_id")
            .and_then(Value::as_str)
            .expect("session id");
        let started = start_turn_payload(
            &config,
            session_id,
            &python_json_dumps(&json!({
                "input": "run approved command",
                "permission": "PLAN_ONLY",
                "tool_call": {
                    "call_id": "call_bash",
                    "name": "bash",
                    "input": {"command": "printf approved"}
                }
            })),
        )
        .expect("start turn");
        assert_eq!(started["status"], "waiting_approval");
        let approval = started["events"]
            .as_array()
            .expect("events")
            .iter()
            .find(|event| event["method"] == "turn/approval_requested")
            .and_then(|event| event["params"]["approval"].as_object())
            .cloned()
            .expect("approval");
        let turn_id = approval
            .get("turn_id")
            .and_then(Value::as_str)
            .expect("turn id");
        let request_id = approval
            .get("request_id")
            .and_then(Value::as_str)
            .expect("request id");
        let resolved = respond_approval_payload(
            &config,
            &format!("/api/turns/{turn_id}/approvals/{request_id}"),
            &python_json_dumps(&json!({"action": "allow", "scope": "once"})),
        )
        .expect("resolve approval");
        let events = resolved["events"].as_array().expect("resolved events");
        assert!(events.iter().any(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["output"] == "approved"
        }));
        assert!(events.iter().any(|event| {
            event["method"] == "turn/completed" && event["params"]["status"] == "completed"
        }));

        let _ = fs::remove_dir_all(root);
    }
}
