//! App Bridge protocol and runtime state for the Rust rewrite.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const MAX_TUI_CONTROL_QUEUE: usize = 100;
pub const UNAUTHORIZED_WWW_AUTHENTICATE: &str = "Bearer realm=\"openagent-app-bridge\"";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn core_crate_name() -> &'static str {
    openagent_core::crate_name()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AppEvent {
    pub sequence: u64,
    pub method: String,
    pub params: Value,
    pub created_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_sequence: Option<u64>,
}

impl AppEvent {
    #[must_use]
    pub fn new(
        sequence: u64,
        method: impl Into<String>,
        params: Value,
        created_at_ms: u64,
    ) -> Self {
        Self {
            sequence,
            method: method.into(),
            params: json_safe(params),
            created_at_ms,
            global_sequence: None,
        }
    }

    #[must_use]
    pub fn with_global_sequence(mut self, global_sequence: u64) -> Self {
        self.global_sequence = Some(global_sequence);
        self
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TuiControlRequest {
    pub path: String,
    pub body: Value,
}

impl TuiControlRequest {
    #[must_use]
    pub fn new(path: impl Into<String>, body: Value) -> Self {
        Self {
            path: path.into(),
            body: json_safe(body),
        }
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SseReplayEvent {
    pub id: String,
    pub event: String,
    pub data: Value,
}

impl SseReplayEvent {
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

#[derive(Clone, Debug, Default)]
pub struct TuiControlQueue {
    requests: VecDeque<TuiControlRequest>,
    responses: VecDeque<Value>,
}

impl TuiControlQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(
        &mut self,
        path: impl Into<String>,
        body: Value,
    ) -> Result<TuiControlRequest, String> {
        if self.requests.len() >= MAX_TUI_CONTROL_QUEUE {
            return Err("TUI control queue is full".to_string());
        }
        let request = TuiControlRequest::new(path, body);
        self.requests.push_back(request.clone());
        Ok(request)
    }

    pub fn pop_next_request(&mut self) -> Option<TuiControlRequest> {
        self.requests.pop_front()
    }

    pub fn record_response(&mut self, payload: Value) -> Value {
        self.responses.push_back(payload.clone());
        payload
    }

    pub fn next_response(&mut self) -> Option<Value> {
        self.responses.pop_front()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TurnRecord {
    pub id: String,
    pub session_id: String,
    pub input: String,
    pub created_at_ms: u64,
    pub status: String,
    pub final_answer: String,
    pub error: Option<String>,
    pub trace: Option<Value>,
    pub interrupt_requested: bool,
    pub events: Vec<AppEvent>,
    pub pending_approval_count: u64,
}

impl TurnRecord {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        session_id: impl Into<String>,
        input: impl Into<String>,
        created_at_ms: u64,
    ) -> Self {
        Self {
            id: id.into(),
            session_id: session_id.into(),
            input: input.into(),
            created_at_ms,
            status: "queued".to_string(),
            final_answer: String::new(),
            error: None,
            trace: None,
            interrupt_requested: false,
            events: Vec::new(),
            pending_approval_count: 0,
        }
    }

    #[must_use]
    pub fn request_interrupt(&mut self, created_at_ms: u64) -> Option<AppEvent> {
        if matches!(self.status.as_str(), "completed" | "failed" | "interrupted") {
            return None;
        }
        if self.interrupt_requested {
            return None;
        }
        self.interrupt_requested = true;
        self.status = "interrupting".to_string();
        let event = lifecycle_event(
            self.events.len() as u64 + 1,
            "turn/interrupt_requested",
            &self.session_id,
            Some(&self.id),
            json!({"status": self.status}),
            created_at_ms,
        );
        self.events.push(event.clone());
        Some(event)
    }

    #[must_use]
    pub fn to_runtime_value(&self) -> Value {
        json!({
            "id": self.id,
            "session_id": self.session_id,
            "status": self.status,
            "created_at_ms": self.created_at_ms,
            "final_answer": self.final_answer,
            "error": self.error,
            "trace": self.trace,
            "event_count": self.events.len(),
            "interrupt_requested": self.interrupt_requested,
            "pending_approval_count": self.pending_approval_count,
        })
    }
}

#[must_use]
pub fn stream_event_to_app_method(event_type: &str) -> &'static str {
    match event_type {
        "step-start" => "item/step/started",
        "step-finish" => "item/step/completed",
        "text-start" => "item/agentMessage/started",
        "text-delta" => "item/agentMessage/delta",
        "text-end" => "item/agentMessage/completed",
        "tool-call" => "item/toolCall/started",
        "tool-result" => "item/toolCall/completed",
        "runtime-warning" => "runtime/warning",
        "patch" => "item/patch/detected",
        "question-request" => "item/question/requested",
        "error" => "turn/error",
        _ => "item/event",
    }
}

#[must_use]
pub fn stream_event_to_app_event(
    event: Value,
    sequence: u64,
    thread_id: &str,
    turn_id: &str,
    created_at_ms: u64,
) -> AppEvent {
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    AppEvent::new(
        sequence,
        stream_event_to_app_method(&event_type),
        json!({
            "thread_id": thread_id,
            "turn_id": turn_id,
            "source": "openagent",
            "event_type": event_type,
            "event": event,
        }),
        created_at_ms,
    )
}

#[must_use]
pub fn lifecycle_event(
    sequence: u64,
    method: &str,
    thread_id: &str,
    turn_id: Option<&str>,
    params: Value,
    created_at_ms: u64,
) -> AppEvent {
    let mut payload = object_from_value(params);
    payload.insert(
        "thread_id".to_string(),
        Value::String(thread_id.to_string()),
    );
    if let Some(turn_id) = turn_id {
        payload.insert("turn_id".to_string(), Value::String(turn_id.to_string()));
    }
    AppEvent::new(sequence, method, Value::Object(payload), created_at_ms)
}

#[must_use]
pub fn is_authenticated_app_path(path: &str) -> bool {
    path.starts_with("/api/") || path.starts_with("/tui/")
}

#[must_use]
pub fn authorize_api_request(auth_token: Option<&str>, authorization: Option<&str>) -> bool {
    let Some(token) = auth_token.filter(|value| !value.is_empty()) else {
        return true;
    };
    authorization.is_some_and(|actual| actual == format!("Bearer {token}"))
}

#[must_use]
pub fn unauthorized_response_payload() -> Value {
    json!({
        "status": 401,
        "headers": {"WWW-Authenticate": UNAUTHORIZED_WWW_AUTHENTICATE},
        "json": {"error": "unauthorized"},
    })
}

#[must_use]
pub fn health_payload(serve_static: bool, auth_required: bool) -> Value {
    json!({
        "ok": true,
        "service": "openagent-app-server",
        "ui_enabled": serve_static,
        "auth_required": auth_required,
    })
}

pub fn parse_turn_approval_path(path: &str) -> Result<(String, String), String> {
    let raw = path.strip_prefix("/api/turns/").unwrap_or(path);
    let Some((turn_id, request_id)) = raw.split_once("/approvals/") else {
        return Err(
            "approval path must be /api/turns/{turn_id}/approvals/{request_id}".to_string(),
        );
    };
    let turn_id = turn_id.trim_matches('/');
    let request_id = request_id.trim_matches('/');
    if turn_id.is_empty() || request_id.is_empty() {
        return Err(
            "approval path must be /api/turns/{turn_id}/approvals/{request_id}".to_string(),
        );
    }
    Ok((turn_id.to_string(), request_id.to_string()))
}

pub fn publish_to_control(payload: &Value) -> Result<(String, Value), String> {
    let topic = ["type", "topic", "event", "method"]
        .iter()
        .find_map(|key| payload.get(*key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "publish type is required".to_string())?;
    let raw_payload = payload.get("properties").or_else(|| payload.get("payload"));
    let params = if let Some(Value::Object(object)) = raw_payload {
        Value::Object(object.clone())
    } else {
        let mut object = Map::new();
        if let Some(source) = payload.as_object() {
            for (key, value) in source {
                if !matches!(
                    key.as_str(),
                    "type" | "topic" | "event" | "method" | "properties" | "payload"
                ) {
                    object.insert(key.clone(), value.clone());
                }
            }
        }
        Value::Object(object)
    };

    match topic {
        "tui.prompt.append" => Ok((
            "prompt.append".to_string(),
            json!({"text": required_string(&params, "text")?}),
        )),
        "tui.command.execute" => Ok((
            "command.execute".to_string(),
            json!({"command": required_string(&params, "command")?}),
        )),
        "tui.toast.show" => {
            let mut result = Map::new();
            result.insert(
                "message".to_string(),
                Value::String(required_string(&params, "message")?),
            );
            if let Some(object) = params.as_object() {
                for key in ["title", "variant", "duration"] {
                    if let Some(value) = object.get(key)
                        && !value.is_null()
                    {
                        result.insert(key.to_string(), value.clone());
                    }
                }
            }
            Ok(("toast.show".to_string(), Value::Object(result)))
        }
        "tui.session.select" => Ok((
            "session.select".to_string(),
            json!({"sessionID": required_string(&params, "sessionID")?}),
        )),
        _ => Err(format!("unsupported publish type: {topic}")),
    }
}

pub fn tui_control_request_for_path(
    path: &str,
    payload: &Value,
) -> Result<TuiControlRequest, String> {
    let body = match path {
        "/tui/append-prompt" => json!({"text": required_string(payload, "text")?}),
        "/tui/submit-prompt" | "/tui/clear-prompt" | "/tui/open-help" | "/tui/open-sessions"
        | "/tui/open-themes" | "/tui/open-models" => json!({}),
        "/tui/execute-command" => json!({"command": required_string(payload, "command")?}),
        "/tui/show-toast" => validate_toast_payload(payload)?,
        "/tui/select-session" => json!({"sessionID": required_string(payload, "sessionID")?}),
        "/tui/publish" => {
            let _ = publish_to_control(payload)?;
            payload.clone()
        }
        _ => return Err("unknown endpoint".to_string()),
    };
    Ok(TuiControlRequest::new(path, body))
}

#[must_use]
pub fn control_next_payload(request: Option<&TuiControlRequest>) -> Value {
    request.map_or_else(
        || json!({"path": "", "body": null}),
        TuiControlRequest::to_value,
    )
}

#[must_use]
pub fn record_control_response_payload(payload: Value) -> Value {
    json!({"ok": true, "response": payload})
}

#[must_use]
pub fn replay_global_events(events: &[AppEvent], last_sequence: u64) -> Vec<SseReplayEvent> {
    events
        .iter()
        .filter_map(|event| {
            let global_sequence = event.global_sequence?;
            (global_sequence > last_sequence).then(|| SseReplayEvent {
                id: global_sequence.to_string(),
                event: event.method.clone(),
                data: event.to_value(),
            })
        })
        .collect()
}

#[must_use]
pub fn ping_comment_frame() -> &'static str {
    ": ping\n\n"
}

#[must_use]
pub fn app_bridge_protocol_fixture() -> Value {
    let event_types = [
        "step-start",
        "step-finish",
        "text-start",
        "text-delta",
        "text-end",
        "tool-call",
        "tool-result",
        "runtime-warning",
        "patch",
        "question-request",
        "error",
        "unknown",
    ];
    let method_map = event_types
        .into_iter()
        .map(|event_type| {
            (
                event_type.to_string(),
                Value::String(stream_event_to_app_method(event_type).to_string()),
            )
        })
        .collect::<Map<_, _>>();
    let wrapped = stream_event_to_app_event(
        json!({"type": "tool-call", "name": "ls", "input": {"path": "."}, "call_id": "call_1"}),
        3,
        "session_1",
        "turn_1",
        1_781_842_000_003,
    );
    let lifecycle = lifecycle_event(
        1,
        "turn/started",
        "session_1",
        Some("turn_1"),
        json!({"status": "running", "input": "hello"}),
        1_781_842_000_001,
    );

    json!({
        "method_map": method_map,
        "wrapped_tool_call": wrapped.to_value(),
        "lifecycle_started": lifecycle.to_value(),
        "tui_control_request": TuiControlRequest::new("/tui/append-prompt", json!({"text": "hello"})).to_value(),
    })
}

#[must_use]
pub fn app_bridge_server_fixture() -> Value {
    let global_events = fixture_global_events();
    let replay_after_query = replay_global_events(&global_events, 1)
        .into_iter()
        .map(|event| event.to_value())
        .collect::<Vec<_>>();
    let replay_after_header = replay_global_events(&global_events, 2)
        .into_iter()
        .map(|event| event.to_value())
        .collect::<Vec<_>>();
    let unsupported_publish_error =
        publish_to_control(&json!({"type": "tui.unknown", "properties": {}}))
            .err()
            .unwrap_or_default();
    let invalid_approval_path = parse_turn_approval_path("/api/turns//approvals/")
        .err()
        .unwrap_or_default();
    let mut turn = TurnRecord::new("turn_1", "session_1", "hello", 1_781_842_000_200);
    turn.status = "running".to_string();
    let interrupt_event = turn
        .request_interrupt(1_781_842_000_201)
        .map_or_else(|| json!({}), |event| event.to_value());
    let requested_approval = lifecycle_event(
        2,
        "turn/approval_requested",
        "session_1",
        Some("turn_1"),
        json!({
            "status": "waiting_approval",
            "approval": {
                "request_id": "approval_1",
                "session_id": "session_1",
                "turn_id": "turn_1",
                "tool_name": "write",
                "tool_input": {"file_path": "blocked.txt"},
                "call_id": "call_1",
                "created_at_ms": 1781842000202u64,
            },
        }),
        1_781_842_000_202,
    );
    let resolved_approval = lifecycle_event(
        3,
        "turn/approval_resolved",
        "session_1",
        Some("turn_1"),
        json!({
            "status": "running",
            "approval": {
                "request_id": "approval_1",
                "session_id": "session_1",
                "turn_id": "turn_1",
                "tool_name": "write",
                "tool_input": {"file_path": "blocked.txt"},
                "call_id": "call_1",
                "created_at_ms": 1781842000202u64,
                "action": "deny",
            },
        }),
        1_781_842_000_203,
    );

    json!({
        "health": health_payload(false, true),
        "auth": {
            "authenticated_paths": {
                "/api/health": is_authenticated_app_path("/api/health"),
                "/tui/append-prompt": is_authenticated_app_path("/tui/append-prompt"),
                "/": is_authenticated_app_path("/"),
            },
            "expected_header": "Bearer server-secret",
            "unauthorized": unauthorized_response_payload(),
        },
        "sse": {
            "replay_after_query_sequence_1": replay_after_query,
            "replay_after_last_event_id_2": replay_after_header,
            "ping_comment": ping_comment_frame(),
        },
        "approval_path": {
            "valid": parse_turn_approval_path("/api/turns/turn_123/approvals/approval_456")
                .map(|(turn_id, request_id)| json!([turn_id, request_id]))
                .unwrap_or_else(|_| json!([])),
            "invalid_error": invalid_approval_path,
        },
        "control_routes": {
            "cases": fixture_control_cases(),
            "publish_samples": fixture_publish_samples(),
            "unsupported_publish_error": unsupported_publish_error,
            "empty_next": control_next_payload(None),
            "record_response": record_control_response_payload(json!(["ok", {"applied": true}])),
        },
        "runtime": {
            "interrupt_event": interrupt_event,
            "turn_after_interrupt": turn.to_runtime_value(),
            "approval_requested": requested_approval.to_value(),
            "approval_resolved": resolved_approval.to_value(),
        },
    })
}

fn fixture_global_events() -> Vec<AppEvent> {
    vec![
        AppEvent::new(
            1,
            "turn/started",
            json!({"thread_id": "session_1", "turn_id": "turn_1", "status": "running"}),
            1_781_842_000_101,
        )
        .with_global_sequence(1),
        AppEvent::new(
            2,
            "turn/completed",
            json!({"thread_id": "session_1", "turn_id": "turn_1", "status": "completed", "final_answer": "done"}),
            1_781_842_000_102,
        )
        .with_global_sequence(2),
        AppEvent::new(
            1,
            "turn/started",
            json!({"thread_id": "session_1", "turn_id": "turn_2", "status": "running"}),
            1_781_842_000_103,
        )
        .with_global_sequence(3),
    ]
}

fn fixture_control_cases() -> Vec<Value> {
    [
        ("/tui/append-prompt", json!({"text": "hello"})),
        ("/tui/submit-prompt", json!({})),
        ("/tui/clear-prompt", json!({})),
        ("/tui/open-help", json!({})),
        ("/tui/open-sessions", json!({})),
        ("/tui/open-themes", json!({})),
        ("/tui/open-models", json!({})),
        ("/tui/execute-command", json!({"command": "status"})),
        (
            "/tui/show-toast",
            json!({"title": "Hi", "message": "Saved", "variant": "success", "duration": 1.5}),
        ),
        (
            "/tui/publish",
            json!({"type": "tui.command.execute", "properties": {"command": "help"}}),
        ),
        (
            "/tui/select-session",
            json!({"sessionID": "session_existing"}),
        ),
    ]
    .into_iter()
    .map(|(path, payload)| {
        let queued = tui_control_request_for_path(path, &payload)
            .map(|request| request.to_value())
            .unwrap_or_else(|error| json!({"error": error}));
        json!({"path": path, "payload": payload, "queued": queued})
    })
    .collect()
}

fn fixture_publish_samples() -> Value {
    let samples = [
        (
            "append",
            json!({"type": "tui.prompt.append", "properties": {"text": "hello"}}),
        ),
        (
            "command",
            json!({"topic": "tui.command.execute", "payload": {"command": "status"}}),
        ),
        (
            "toast",
            json!({"event": "tui.toast.show", "payload": {"title": "Saved", "message": "Done", "variant": "success", "duration": 1.25}}),
        ),
        (
            "session",
            json!({"method": "tui.session.select", "payload": {"sessionID": "session_existing"}}),
        ),
    ];
    let mut object = Map::new();
    for (name, payload) in samples {
        let value = publish_to_control(&payload)
            .map(|(action, params)| json!({"action": action, "params": params}))
            .unwrap_or_else(|error| json!({"error": error}));
        object.insert(name.to_string(), value);
    }
    Value::Object(object)
}

fn validate_toast_payload(payload: &Value) -> Result<Value, String> {
    let message = required_string(payload, "message")?;
    let mut object = Map::new();
    object.insert("message".to_string(), Value::String(message));
    for key in ["title", "variant"] {
        if let Some(value) = payload.get(key)
            && !value.is_null()
        {
            if !value.is_string() {
                return Err(format!("{key} must be a string"));
            }
            object.insert(key.to_string(), value.clone());
        }
    }
    if let Some(value) = payload.get("duration")
        && !value.is_null()
    {
        if !value.is_number() {
            return Err("duration must be a number".to_string());
        }
        object.insert("duration".to_string(), value.clone());
    }
    Ok(Value::Object(object))
}

fn required_string(payload: &Value, key: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| format!("{key} is required"))
}

fn object_from_value(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object,
        _ => Map::new(),
    }
}

fn json_safe(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(json_safe).collect()),
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, json_safe(value)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_core_crate() {
        assert_eq!(crate_name(), "openagent-app-server");
        assert_eq!(core_crate_name(), "openagent-core");
    }
}
