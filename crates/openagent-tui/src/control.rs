use serde_json::{Map, Value, json};

use crate::config::is_valid_color_scheme;

#[must_use]
pub fn normalize_control_action(action: &str) -> String {
    match action {
        "append-prompt" => "prompt.append",
        "submit-prompt" => "prompt.submit",
        "clear-prompt" => "prompt.clear",
        "open-help" => "help.open",
        "open-sessions" => "sessions.open",
        "open-themes" => "theme.open",
        "open-theme-schemes" | "open-theme-scheme" | "open-color-schemes" | "open-color-scheme" => {
            "theme.scheme.open"
        }
        "open-models" => "model.open",
        "open-agents" => "agent.open",
        "open-variants" => "variant.open",
        "open-thinking" => "thinking.open",
        "open-palette" => "palette.open",
        "open-files" => "file.open",
        "select-session" => "session.select",
        "rename-session" => "session.rename",
        "archive-session" => "session.archive",
        "unarchive-session" => "session.unarchive",
        "delete-session" => "session.delete",
        "fork-session" => "session.fork",
        "session-children" | "open-children" => "session.children",
        "parent-session" => "session.parent",
        "share-session" => "session.share",
        "unshare-session" => "session.unshare",
        "compact-session" => "session.compact",
        "session-details" => "session.details",
        "undo-session" => "session.undo",
        "redo-session" => "session.redo",
        "select-model" => "model.select",
        "select-agent" => "agent.select",
        "select-variant" => "variant.select",
        "select-thinking" => "thinking.select",
        "select-theme" => "theme.select",
        "select-theme-scheme" | "select-color-scheme" => "theme.scheme.select",
        "cycle-theme-scheme" | "cycle-color-scheme" => "theme.scheme.cycle",
        "select-file" | "attach-file" => "file.select",
        "show-toast" => "toast.show",
        "execute-command" => "command.execute",
        "execute-palette" => "palette.execute",
        "respond-approval" => "approval.respond",
        "reply-question" => "question.reply",
        "dismiss-question" => "question.dismiss",
        other => other,
    }
    .to_string()
}

pub(crate) fn control_string_field(params: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
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
    if let Some(scheme) = topic
        .strip_prefix("theme.scheme.")
        .or_else(|| topic.strip_prefix("tui.theme.scheme."))
        .filter(|scheme| is_valid_color_scheme(scheme))
    {
        let mut object = body.as_object().cloned().unwrap_or_default();
        object
            .entry("scheme".to_string())
            .or_insert_with(|| json!(scheme));
        return ("theme.scheme.select".to_string(), Value::Object(object));
    }
    let action = match topic {
        "tui.prompt.append" => "prompt.append",
        "tui.command.execute" => "command.execute",
        "tui.toast.show" => "toast.show",
        "tui.session.select" => "session.select",
        "tui.session.rename" => "session.rename",
        "tui.session.archive" => "session.archive",
        "tui.session.unarchive" => "session.unarchive",
        "tui.session.delete" => "session.delete",
        "tui.session.fork" => "session.fork",
        "tui.session.children" => "session.children",
        "tui.session.parent" => "session.parent",
        "tui.session.share" => "session.share",
        "tui.session.unshare" => "session.unshare",
        "tui.session.compact" => "session.compact",
        "tui.session.details" => "session.details",
        "tui.session.undo" => "session.undo",
        "tui.session.redo" => "session.redo",
        "tui.model.open" => "model.open",
        "tui.model.select" => "model.select",
        "tui.agent.open" => "agent.open",
        "tui.agent.select" => "agent.select",
        "tui.variant.open" => "variant.open",
        "tui.variant.select" => "variant.select",
        "tui.thinking.open" => "thinking.open",
        "tui.thinking.select" => "thinking.select",
        "tui.theme.open" => "theme.open",
        "tui.theme.select" => "theme.select",
        "tui.theme.scheme.open" => "theme.scheme.open",
        "tui.theme.scheme.select" => "theme.scheme.select",
        "tui.theme.scheme.cycle" => "theme.scheme.cycle",
        "theme.scheme.open" => "theme.scheme.open",
        "theme.scheme.select" => "theme.scheme.select",
        "theme.scheme.cycle" => "theme.scheme.cycle",
        "tui.file.open" => "file.open",
        "tui.file.select" | "tui.file.attach" => "file.select",
        other => other,
    };
    (action.to_string(), body)
}

pub(crate) fn action_map_fixture() -> Value {
    let mut object = Map::new();
    for name in [
        "append-prompt",
        "submit-prompt",
        "clear-prompt",
        "open-help",
        "open-sessions",
        "open-themes",
        "open-theme-schemes",
        "open-color-schemes",
        "open-models",
        "open-agents",
        "open-variants",
        "open-thinking",
        "open-palette",
        "open-files",
        "select-session",
        "rename-session",
        "archive-session",
        "unarchive-session",
        "delete-session",
        "fork-session",
        "session-children",
        "parent-session",
        "share-session",
        "unshare-session",
        "compact-session",
        "session-details",
        "undo-session",
        "redo-session",
        "select-model",
        "select-agent",
        "select-variant",
        "select-thinking",
        "select-theme",
        "select-theme-scheme",
        "select-color-scheme",
        "cycle-theme-scheme",
        "cycle-color-scheme",
        "select-file",
        "attach-file",
        "show-toast",
        "execute-command",
        "execute-palette",
        "custom.action",
    ] {
        object.insert(
            name.to_string(),
            Value::String(normalize_control_action(name)),
        );
    }
    Value::Object(object)
}
