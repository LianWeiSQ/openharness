//! App Bridge client-side state for the Rust rewrite.

use std::collections::BTreeSet;

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
