//! Terminal UI state for the Rust rewrite.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

const BUILTIN_COMMANDS: &[(&str, &str)] = &[
    ("/help", "show TUI commands"),
    ("/sessions", "open recent session picker"),
    ("/resume <id>", "resume a session by id or unique prefix"),
    (
        "/transcript [limit]",
        "show recent messages from the current session",
    ),
    ("/new", "start a new session"),
    ("/clear", "clear the visible timeline"),
    ("/status", "show current session, turn, and model status"),
    ("/commands", "list project/global custom commands"),
];

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn command_name() -> &'static str {
    "openagent-tui"
}

#[must_use]
pub fn client_crate_name() -> &'static str {
    openagent_app_server_client::crate_name()
}

#[must_use]
pub fn server_crate_name() -> &'static str {
    openagent_app_server::crate_name()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TimelineLine {
    pub kind: String,
    pub text: String,
    pub important: bool,
}

impl TimelineLine {
    #[must_use]
    pub fn new(kind: impl Into<String>, text: impl Into<String>, important: bool) -> Self {
        Self {
            kind: kind.into(),
            text: text.into(),
            important,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TuiState {
    pub input_buffer: String,
    pub status: String,
    pub timeline: Vec<TimelineLine>,
    pub session_id: Option<String>,
}

impl TuiState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            input_buffer: String::new(),
            status: "idle".to_string(),
            timeline: Vec::new(),
            session_id: None,
        }
    }

    pub fn apply_control_request(&mut self, request: &Value) -> Value {
        let path = request
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut action;
        let mut params;
        if !path.is_empty() {
            action = normalize_control_action(path.trim_start_matches("/tui/").trim_matches('/'));
            params = object_value(request.get("body"));
            if action == "publish" {
                (action, params) = control_publish_to_action(&params);
            }
        } else {
            action = request
                .get("action")
                .or_else(|| request.get("type"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            params = object_value(request.get("params"));
        }
        action = normalize_control_action(&action);

        match action.as_str() {
            "prompt.append" => {
                let text = params
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.input_buffer.push_str(text);
                self.status = "prompt updated".to_string();
                json!({"applied": true, "action": action})
            }
            "prompt.submit" => {
                let submitted = self.submit();
                json!({"applied": submitted, "action": action})
            }
            "prompt.clear" => {
                self.input_buffer.clear();
                self.status = "prompt cleared".to_string();
                json!({"applied": true, "action": action})
            }
            "help.open" => {
                self.show_help();
                json!({"applied": true, "action": action})
            }
            "sessions.open" => {
                self.status = "session picker".to_string();
                self.timeline.push(TimelineLine::new(
                    "status",
                    "session picker opened. Use Up/Down or j/k, Enter to resume, Esc to close.",
                    true,
                ));
                json!({"applied": true, "action": action})
            }
            "session.select" => {
                let session_id = params
                    .get("sessionID")
                    .or_else(|| params.get("session_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if session_id.is_empty() {
                    self.timeline.push(TimelineLine::new(
                        "error",
                        "control request missing sessionID",
                        true,
                    ));
                    self.status = "control invalid".to_string();
                    return json!({"applied": false, "action": action, "error": "sessionID is required"});
                }
                self.session_id = Some(session_id.to_string());
                self.input_buffer.clear();
                self.timeline.clear();
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("resumed session: {session_id}"),
                    true,
                ));
                self.status = "session resumed".to_string();
                json!({"applied": true, "action": action})
            }
            "toast.show" => {
                let message = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if message.is_empty() {
                    self.status = "control invalid".to_string();
                    return json!({"applied": false, "action": action, "error": "message is required"});
                }
                let title = params
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("toast");
                let variant = params
                    .get("variant")
                    .and_then(Value::as_str)
                    .unwrap_or("status")
                    .to_ascii_lowercase();
                let kind = if matches!(variant.as_str(), "error" | "danger") {
                    "error"
                } else if matches!(variant.as_str(), "warn" | "warning") {
                    "warning"
                } else {
                    "status"
                };
                self.timeline
                    .push(TimelineLine::new(kind, format!("{title}: {message}"), true));
                self.status = title.to_string();
                json!({"applied": true, "action": action})
            }
            "command.execute" => {
                let command = params
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if command.is_empty() {
                    self.status = "control invalid".to_string();
                    return json!({"applied": false, "action": action, "error": "command is required"});
                }
                self.input_buffer = if command.starts_with('/') {
                    command.to_string()
                } else {
                    format!("/{command}")
                };
                let submitted = self.submit();
                json!({"applied": submitted, "action": action})
            }
            unsupported
                if unsupported.starts_with("model.")
                    || unsupported.starts_with("theme.")
                    || unsupported.starts_with("palette.") =>
            {
                self.timeline.push(TimelineLine::new(
                    "warning",
                    format!("TUI control unsupported: {unsupported}"),
                    true,
                ));
                self.status = "control unsupported".to_string();
                json!({"applied": false, "action": action, "unsupported": true})
            }
            _ => {
                self.timeline.push(TimelineLine::new(
                    "warning",
                    format!(
                        "unknown TUI control: {}",
                        if action.is_empty() { "-" } else { &action }
                    ),
                    true,
                ));
                self.status = "control unknown".to_string();
                json!({"applied": false, "action": action, "unsupported": true})
            }
        }
    }

    pub fn submit(&mut self) -> bool {
        let raw_text = self.input_buffer.trim().to_string();
        if raw_text == "/help" || raw_text == "/?" || raw_text == "/" {
            self.show_help();
            self.input_buffer.clear();
            return false;
        }
        if raw_text.is_empty() {
            return false;
        }
        self.input_buffer.clear();
        self.status = "running".to_string();
        self.timeline
            .push(TimelineLine::new("user", format!("> {raw_text}"), true));
        true
    }

    fn show_help(&mut self) {
        let lines = BUILTIN_COMMANDS
            .iter()
            .map(|(name, description)| format!("{name} - {description}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.timeline.push(TimelineLine::new(
            "status",
            format!("built-in commands:\n{lines}"),
            true,
        ));
        self.status = "help listed".to_string();
    }
}

#[must_use]
pub fn normalize_control_action(action: &str) -> String {
    match action {
        "append-prompt" => "prompt.append",
        "submit-prompt" => "prompt.submit",
        "clear-prompt" => "prompt.clear",
        "open-help" => "help.open",
        "open-sessions" => "sessions.open",
        "open-themes" => "theme.open",
        "open-models" => "model.open",
        "select-session" => "session.select",
        "show-toast" => "toast.show",
        "execute-command" => "command.execute",
        other => other,
    }
    .to_string()
}

#[must_use]
pub fn control_publish_to_action(params: &Value) -> (String, Value) {
    let topic = params
        .get("type")
        .or_else(|| params.get("topic"))
        .or_else(|| params.get("event"))
        .or_else(|| params.get("method"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let payload = params.get("properties").or_else(|| params.get("payload"));
    let body = if let Some(Value::Object(object)) = payload {
        Value::Object(object.clone())
    } else {
        let mut object = Map::new();
        if let Some(source) = params.as_object() {
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
    let action = match topic {
        "tui.prompt.append" => "prompt.append",
        "tui.command.execute" => "command.execute",
        "tui.toast.show" => "toast.show",
        "tui.session.select" => "session.select",
        other => other,
    };
    (action.to_string(), body)
}

#[must_use]
pub fn tui_control_fixture() -> Value {
    let mut state = TuiState::new();
    let requests = vec![
        json!({"path": "/tui/append-prompt", "body": {"text": "hello"}}),
        json!({"path": "/tui/publish", "body": {"type": "tui.prompt.append", "properties": {"text": " next"}}}),
        json!({"path": "/tui/show-toast", "body": {"title": "Saved", "message": "Session selected", "variant": "success"}}),
        json!({"path": "/tui/execute-command", "body": {"command": "help"}}),
        json!({"path": "/tui/open-themes", "body": {}}),
        json!({"path": "/tui/clear-prompt", "body": {}}),
    ];
    let steps = requests
        .into_iter()
        .map(|request| {
            let result = state.apply_control_request(&request);
            json!({
                "request": request,
                "result": result,
                "status": state.status,
                "input_buffer": state.input_buffer,
                "timeline": state.timeline,
            })
        })
        .collect::<Vec<_>>();
    let mut invalid_state = TuiState::new();
    let invalid_select =
        invalid_state.apply_control_request(&json!({"path": "/tui/select-session", "body": {}}));

    json!({
        "action_map": action_map_fixture(),
        "steps": steps,
        "invalid_select": {
            "result": invalid_select,
            "status": invalid_state.status,
            "timeline": invalid_state.timeline,
        },
    })
}

fn action_map_fixture() -> Value {
    let mut object = Map::new();
    for name in [
        "append-prompt",
        "submit-prompt",
        "clear-prompt",
        "open-help",
        "open-sessions",
        "open-themes",
        "open-models",
        "select-session",
        "show-toast",
        "execute-command",
        "custom.action",
    ] {
        object.insert(
            name.to_string(),
            Value::String(normalize_control_action(name)),
        );
    }
    Value::Object(object)
}

fn object_value(value: Option<&Value>) -> Value {
    value
        .and_then(Value::as_object)
        .cloned()
        .map(Value::Object)
        .unwrap_or_else(|| json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_command_boundary() {
        assert_eq!(crate_name(), "openagent-tui");
        assert_eq!(command_name(), "openagent-tui");
        assert_eq!(client_crate_name(), "openagent-app-server-client");
        assert_eq!(server_crate_name(), "openagent-app-server");
    }
}
