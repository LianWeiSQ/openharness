//! App Bridge client-side state for the Rust rewrite.

use std::{collections::BTreeSet, path::Path, time::Duration};

use openagent_app_server::AppEvent;
use serde_json::{Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

const TERMINAL_METHODS: &[&str] = &["turn/completed", "turn/failed", "turn/interrupted"];
const TERMINAL_STATUSES: &[&str] = &["completed", "failed", "interrupted"];

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn protocol_crate_name() -> &'static str {
    openagent_protocol::crate_name()
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RemoteEventKey {
    Global(u64),
    Turn {
        turn_id: String,
        sequence: u64,
        method: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteTurnRecord {
    pub id: String,
    pub session_id: String,
    pub status: String,
    pub final_answer: String,
    pub error: Option<String>,
    pub trace: Option<Value>,
    pub events: Vec<AppEvent>,
    seen_event_keys: BTreeSet<RemoteEventKey>,
}

impl RemoteTurnRecord {
    #[must_use]
    pub fn new(id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            session_id: session_id.into(),
            status: "queued".to_string(),
            final_answer: String::new(),
            error: None,
            trace: None,
            events: Vec::new(),
            seen_event_keys: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn from_payload(payload: &Value, session_id: &str) -> Self {
        Self {
            id: string_field(payload, "id"),
            session_id: string_field(payload, "session_id")
                .if_empty_then(|| session_id.to_string()),
            status: string_field(payload, "status").if_empty_then(|| "queued".to_string()),
            final_answer: string_field(payload, "final_answer"),
            error: optional_string_field(payload, "error"),
            trace: payload
                .get("trace")
                .filter(|value| value.is_object())
                .cloned(),
            events: Vec::new(),
            seen_event_keys: BTreeSet::new(),
        }
    }

    pub fn append_event(&mut self, event: AppEvent) -> bool {
        let key = remote_event_key(&event, &self.id);
        if self.seen_event_keys.contains(&key) {
            return false;
        }
        self.seen_event_keys.insert(key);
        self.apply_event(&event);
        self.events.push(event);
        true
    }

    pub fn mark_failed(&mut self, error: impl Into<String>) {
        self.status = "failed".to_string();
        self.error = Some(error.into());
    }

    fn apply_event(&mut self, event: &AppEvent) {
        match event.method.as_str() {
            "turn/approval_requested" => {
                self.status = string_field(&event.params, "status")
                    .if_empty_then(|| "waiting_approval".to_string());
            }
            "turn/approval_resolved" | "turn/started" => {
                self.status =
                    string_field(&event.params, "status").if_empty_then(|| "running".to_string());
            }
            method if TERMINAL_METHODS.contains(&method) => {
                let default_status = match method {
                    "turn/completed" => "completed",
                    "turn/interrupted" => "interrupted",
                    _ => "failed",
                };
                self.status = string_field(&event.params, "status")
                    .if_empty_then(|| default_status.to_string());
                let final_answer = string_field(&event.params, "final_answer");
                if !final_answer.is_empty() {
                    self.final_answer = final_answer;
                }
                if let Some(error) = optional_string_field(&event.params, "error") {
                    self.error = Some(error);
                }
                if let Some(trace) = event.params.get("trace").filter(|value| value.is_object()) {
                    self.trace = Some(trace.clone());
                }
            }
            _ => {}
        }
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATUSES.contains(&self.status.as_str())
    }
}

#[must_use]
pub fn normalize_server_url(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

#[must_use]
pub fn join_server_url(server_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        normalize_server_url(server_url),
        path.trim_start_matches('/')
    )
}

#[must_use]
pub fn quote_path(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            output.push(ch);
        } else {
            output.push('%');
            output.push(hex_digit(byte >> 4));
            output.push(hex_digit(byte & 0x0f));
        }
    }
    output
}

#[must_use]
pub fn auth_header(token: Option<&str>) -> Option<String> {
    token
        .filter(|value| !value.is_empty())
        .map(|value| format!("Bearer {value}"))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoteAuth {
    pub token: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl RemoteAuth {
    #[must_use]
    pub fn bearer(token: impl Into<String>) -> Self {
        Self {
            token: Some(token.into()),
            username: None,
            password: None,
        }
    }

    #[must_use]
    pub fn basic(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            token: None,
            username: Some(username.into()),
            password: Some(password.into()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteRuntimeClient {
    server_url: String,
    auth: RemoteAuth,
    timeout: Duration,
}

impl RemoteRuntimeClient {
    #[must_use]
    pub fn new(server_url: impl Into<String>) -> Self {
        Self {
            server_url: normalize_server_url(&server_url.into()),
            auth: RemoteAuth::default(),
            timeout: Duration::from_secs(5),
        }
    }

    #[must_use]
    pub fn with_auth(mut self, auth: RemoteAuth) -> Self {
        self.auth = auth;
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    #[must_use]
    pub fn auth(&self) -> &RemoteAuth {
        &self.auth
    }

    pub fn health(&self) -> Result<Value, String> {
        self.json("GET", "/api/health", None)
    }

    pub fn models(&self) -> Result<Value, String> {
        self.json("GET", "/api/models", None)
    }

    pub fn agents(&self) -> Result<Value, String> {
        self.json("GET", "/api/agents", None)
    }

    pub fn list_sessions(&self) -> Result<Vec<Value>, String> {
        let payload = self.json("GET", "/api/sessions", None)?;
        Ok(payload
            .get("sessions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub fn get_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("GET", &format!("/api/sessions/{session_id}"), None)
    }

    pub fn search_sessions(&self, query: &str) -> Result<Vec<Value>, String> {
        let path = if query.trim().is_empty() {
            "/api/sessions".to_string()
        } else {
            format!("/api/sessions?query={}", quote_path(query.trim()))
        };
        let payload = self.json("GET", &path, None)?;
        Ok(payload
            .get("sessions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub fn create_session(
        &self,
        workspace: &Path,
        fork_from: Option<&str>,
    ) -> Result<String, String> {
        let mut body = json!({"cwd": workspace.to_string_lossy()});
        if let Some(fork_from) = fork_from.filter(|value| !value.is_empty()) {
            body["fork_from"] = json!(fork_from);
        }
        let payload = self.json("POST", "/api/sessions", Some(body))?;
        session_id_from_payload(&payload)
            .ok_or_else(|| "server did not return a session id".to_string())
    }

    pub fn select_session(
        &self,
        explicit: Option<String>,
        continue_last: bool,
        fork: bool,
        workspace: &Path,
    ) -> Result<String, String> {
        if fork && explicit.is_none() && !continue_last {
            return Err("fork requires an explicit session or continue_last".to_string());
        }
        let base = if let Some(session_id) = explicit {
            Some(session_id)
        } else if continue_last {
            self.list_sessions()?
                .first()
                .and_then(session_id_from_payload)
        } else {
            None
        };
        if !fork && let Some(session_id) = base {
            return Ok(session_id);
        }
        self.create_session(workspace, base.as_deref())
    }

    pub fn start_turn(
        &self,
        session_id: &str,
        prompt: &str,
        mut extra: Value,
    ) -> Result<Value, String> {
        if !extra.is_object() {
            extra = json!({});
        }
        extra["input"] = json!(prompt);
        self.json(
            "POST",
            &format!("/api/sessions/{session_id}/turns"),
            Some(extra),
        )
    }

    pub fn interrupt_turn(&self, turn_id: &str) -> Result<Value, String> {
        self.json("POST", &format!("/api/turns/{turn_id}/interrupt"), None)
    }

    pub fn update_session(&self, session_id: &str, body: Value) -> Result<Value, String> {
        self.json("PATCH", &format!("/api/sessions/{session_id}"), Some(body))
    }

    pub fn delete_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("DELETE", &format!("/api/sessions/{session_id}"), None)
    }

    pub fn children(&self, session_id: &str) -> Result<Vec<Value>, String> {
        let payload = self.json("GET", &format!("/api/sessions/{session_id}/children"), None)?;
        Ok(payload
            .get("children")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub fn share_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("POST", &format!("/api/sessions/{session_id}/share"), None)
    }

    pub fn unshare_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("DELETE", &format!("/api/sessions/{session_id}/share"), None)
    }

    pub fn compact_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("POST", &format!("/api/sessions/{session_id}/compact"), None)
    }

    pub fn session_diff(&self, session_id: &str) -> Result<Value, String> {
        self.json("GET", &format!("/api/sessions/{session_id}/diff"), None)
    }

    pub fn undo_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("POST", &format!("/api/sessions/{session_id}/undo"), None)
    }

    pub fn redo_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("POST", &format!("/api/sessions/{session_id}/redo"), None)
    }

    pub fn turn_events(&self, turn_id: &str, last_event_id: u64) -> Result<Vec<Value>, String> {
        let path = if last_event_id == 0 {
            format!("/api/turns/{turn_id}/events")
        } else {
            format!("/api/turns/{turn_id}/events?last_event_id={last_event_id}")
        };
        self.sse_events(&path)
    }

    pub fn global_events(&self, last_event_id: u64) -> Result<Vec<Value>, String> {
        self.sse_events(&format!("/api/events?last_event_id={last_event_id}"))
    }

    pub fn respond_approval(&self, payload: &Value) -> Result<Value, String> {
        let turn_id = string_field(payload, "turn_id");
        let request_id = string_field(payload, "request_id");
        if turn_id.is_empty() || request_id.is_empty() {
            return Err("approval response requires turn_id and request_id".to_string());
        }
        self.json(
            "POST",
            &format!("/api/turns/{turn_id}/approvals/{request_id}"),
            Some(payload.clone()),
        )
    }

    pub fn respond_question(&self, payload: &Value) -> Result<Value, String> {
        let turn_id = string_field(payload, "turn_id");
        let request_id = string_field(payload, "request_id");
        if turn_id.is_empty() || request_id.is_empty() {
            return Err("question response requires turn_id and request_id".to_string());
        }
        self.json(
            "POST",
            &format!("/api/turns/{turn_id}/questions/{request_id}/reply"),
            Some(payload.clone()),
        )
    }

    pub fn next_tui_control(&self) -> Result<Value, String> {
        self.json("GET", "/tui/control/next", None)
    }

    pub fn record_tui_control_response(&self, payload: &Value) -> Result<Value, String> {
        self.json("POST", "/tui/control/response", Some(payload.clone()))
    }

    pub fn json(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let raw = self.text(method, path, body)?;
        serde_json::from_str(&raw).map_err(|error| format!("server response was not JSON: {error}"))
    }

    pub fn sse_events(&self, path: &str) -> Result<Vec<Value>, String> {
        let raw = self.text("GET", path, None)?;
        parse_sse_response_lines(&raw.lines().collect::<Vec<_>>())
    }

    pub fn text(&self, method: &str, path: &str, body: Option<Value>) -> Result<String, String> {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(self.timeout)
            .build()
            .map_err(|error| error.to_string())?;
        let url = join_server_url(&self.server_url, path);
        let mut request = match method {
            "DELETE" => client.delete(url),
            "GET" => client.get(url),
            "PATCH" => client.patch(url),
            "POST" => client.post(url),
            other => return Err(format!("unsupported HTTP method: {other}")),
        };
        if let Some(token) = self.auth.token.as_deref().filter(|value| !value.is_empty()) {
            request = request.bearer_auth(token);
        } else if let Some(password) = self
            .auth
            .password
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            request = request.basic_auth(
                self.auth.username.as_deref().unwrap_or("openagent"),
                Some(password),
            );
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().map_err(|error| {
            format!(
                "{method} {} failed: {error}",
                join_server_url(&self.server_url, path)
            )
        })?;
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let raw = response.text().map_err(|error| error.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "server returned HTTP {} for {method} {path}: {}",
                status.as_u16(),
                summarize_http_error_body(&raw, &content_type)
            ));
        }
        Ok(raw)
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
pub fn session_id_from_payload(payload: &Value) -> Option<String> {
    payload
        .get("session_id")
        .or_else(|| payload.get("id"))
        .or_else(|| payload.get("session").and_then(|session| session.get("id")))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[must_use]
pub fn turn_id_from_payload(payload: &Value) -> Option<String> {
    payload
        .get("turn_id")
        .or_else(|| payload.get("turn").and_then(|turn| turn.get("id")))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[must_use]
pub fn events_from_payload(payload: &Value) -> Vec<Value> {
    payload
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[must_use]
pub fn event_sequence(event: &Value) -> u64 {
    event
        .get("global_sequence")
        .or_else(|| event.get("sequence"))
        .and_then(Value::as_u64)
        .unwrap_or_default()
}

pub fn app_event_from_value(payload: &Value, default_sequence: u64) -> Result<AppEvent, String> {
    let sequence = payload
        .get("sequence")
        .and_then(Value::as_u64)
        .unwrap_or(default_sequence);
    let method = string_field(payload, "method");
    let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
    let created_at_ms = payload
        .get("created_at_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let global_sequence = payload.get("global_sequence").and_then(Value::as_u64);
    Ok(AppEvent {
        sequence,
        method,
        params,
        created_at_ms,
        global_sequence,
    })
}

#[must_use]
pub fn event_turn_id(event: &AppEvent) -> String {
    if let Some(value) = event.params.get("turn_id").and_then(Value::as_str)
        && !value.is_empty()
    {
        return value.to_string();
    }
    event
        .params
        .get("approval")
        .and_then(Value::as_object)
        .and_then(|approval| approval.get("turn_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[must_use]
pub fn event_session_id(event: &AppEvent) -> String {
    event
        .params
        .get("thread_id")
        .or_else(|| event.params.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[must_use]
pub fn remote_event_key(event: &AppEvent, default_turn_id: &str) -> RemoteEventKey {
    if let Some(global_sequence) = event.global_sequence {
        return RemoteEventKey::Global(global_sequence);
    }
    RemoteEventKey::Turn {
        turn_id: event_turn_id(event).if_empty_then(|| default_turn_id.to_string()),
        sequence: event.sequence,
        method: event.method.clone(),
    }
}

#[must_use]
pub fn remote_event_key_value(key: &RemoteEventKey) -> Value {
    match key {
        RemoteEventKey::Global(sequence) => json!(["global", sequence]),
        RemoteEventKey::Turn {
            turn_id,
            sequence,
            method,
        } => json!(["turn", turn_id, sequence, method]),
    }
}

#[must_use]
pub fn request_shape(method: &str, path: &str, payload: Option<Value>) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("method".to_string(), Value::String(method.to_string()));
    object.insert("path".to_string(), Value::String(path.to_string()));
    if let Some(payload) = payload {
        object.insert("payload".to_string(), payload);
    }
    Value::Object(object)
}

#[must_use]
pub fn app_bridge_client_fixture() -> Value {
    let parsed_event = app_event_from_value(
        &json!({
            "sequence": 4,
            "global_sequence": 12,
            "method": "turn/completed",
            "params": {
                "thread_id": "session_existing",
                "turn_id": "turn_remote",
                "status": "completed",
                "final_answer": "hello remote",
                "trace": {"id": "trace_1"},
            },
            "created_at_ms": 1781842000304u64,
        }),
        99,
    )
    .unwrap_or_else(|_| AppEvent {
        sequence: 0,
        method: String::new(),
        params: json!({}),
        created_at_ms: 0,
        global_sequence: None,
    });
    let key = remote_event_key(&parsed_event, "turn_remote");
    let mut remote_turn = RemoteTurnRecord::new("turn_remote", "session_existing");
    let remote_events = vec![
        AppEvent {
            sequence: 1,
            global_sequence: Some(10),
            method: "turn/started".to_string(),
            params: json!({"thread_id": "session_existing", "turn_id": "turn_remote", "status": "running"}),
            created_at_ms: 1_781_842_000_301,
        },
        AppEvent {
            sequence: 2,
            global_sequence: Some(11),
            method: "turn/approval_requested".to_string(),
            params: json!({
                "thread_id": "session_existing",
                "turn_id": "turn_remote",
                "status": "waiting_approval",
                "approval": {"turn_id": "turn_remote", "request_id": "approval_1", "tool_name": "write"},
            }),
            created_at_ms: 1_781_842_000_302,
        },
        AppEvent {
            sequence: 3,
            global_sequence: None,
            method: "turn/approval_resolved".to_string(),
            params: json!({
                "thread_id": "session_existing",
                "turn_id": "turn_remote",
                "status": "running",
                "approval": {"turn_id": "turn_remote", "request_id": "approval_1", "action": "deny"},
            }),
            created_at_ms: 1_781_842_000_303,
        },
        parsed_event.clone(),
    ];
    let append_results = remote_events
        .into_iter()
        .map(|event| remote_turn.append_event(event))
        .collect::<Vec<_>>();
    let duplicate_result = remote_turn.append_event(AppEvent {
        sequence: 1,
        global_sequence: Some(10),
        method: "turn/started".to_string(),
        params: json!({"thread_id": "session_existing", "turn_id": "turn_remote", "status": "running"}),
        created_at_ms: 1_781_842_000_301,
    });
    let events = remote_turn
        .events
        .iter()
        .map(AppEvent::to_value)
        .collect::<Vec<_>>();

    json!({
        "helpers": {
            "normalize": normalize_server_url("http://127.0.0.1:8787/"),
            "join": join_server_url("http://127.0.0.1:8787/", "/api/sessions"),
            "quote": quote_path("turn/a b"),
            "auth_header": auth_header(Some("secret")).unwrap_or_default(),
        },
        "parsed_event": parsed_event.to_value(),
        "event_ids": {
            "turn": event_turn_id(&parsed_event),
            "session": event_session_id(&parsed_event),
            "key": remote_event_key_value(&key),
        },
        "remote_turn": {
            "append_results": append_results,
            "duplicate_result": duplicate_result,
            "status": remote_turn.status,
            "final_answer": remote_turn.final_answer,
            "trace": remote_turn.trace,
            "events": events,
        },
        "request_shapes": {
            "start_session": request_shape("POST", "/api/sessions", Some(json!({"cwd": "/tmp/openagent-rust-rewrite-fixture-goal11/workspace"}))),
            "start_turn": request_shape("POST", "/api/sessions/session_existing/turns", Some(json!({"input": "hello"}))),
            "interrupt": request_shape("POST", "/api/turns/turn_remote/interrupt", Some(json!({}))),
            "approval": request_shape("POST", "/api/turns/turn_remote/approvals/approval_1", Some(json!({"action": "deny"}))),
            "control_next": request_shape("GET", "/tui/control/next?timeout=0.25", None),
            "control_response": request_shape("POST", "/tui/control/response", Some(json!({"ok": true, "result": {"applied": true}}))),
        },
    })
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + (value - 10)) as char,
        _ => '0',
    }
}

fn string_field(payload: &Value, key: &str) -> String {
    payload
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn optional_string_field(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn summarize_http_error_body(raw: &str, content_type: &str) -> String {
    if raw.trim().is_empty() {
        return "empty response body".to_string();
    }
    if content_type.contains("json")
        && let Ok(value) = serde_json::from_str::<Value>(raw)
    {
        if let Some(error) = value
            .get("error")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return error.to_string();
        }
        return value.to_string();
    }
    raw.lines().take(5).collect::<Vec<_>>().join("\n")
}

trait EmptyStringExt {
    fn if_empty_then<F>(self, fallback: F) -> Self
    where
        F: FnOnce() -> Self;
}

impl EmptyStringExt for String {
    fn if_empty_then<F>(self, fallback: F) -> Self
    where
        F: FnOnce() -> Self,
    {
        if self.is_empty() { fallback() } else { self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_protocol_crate() {
        assert_eq!(crate_name(), "openagent-app-server-client");
        assert_eq!(protocol_crate_name(), "openagent-protocol");
    }
}
