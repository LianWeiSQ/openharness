use super::*;

pub(super) fn remote_sessions_text(sessions: &[Value]) -> String {
    if sessions.is_empty() {
        return "Remote sessions: none\n".to_string();
    }
    let mut text = String::from("Remote sessions:\n");
    for (index, session) in sessions.iter().take(20).enumerate() {
        let id = session
            .get("session_id")
            .or_else(|| session.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let status = session
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let messages = session
            .get("message_count")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let workspace = session
            .get("workspace")
            .and_then(Value::as_str)
            .unwrap_or(".");
        text.push_str(&format!(
            "  {}. {}  status={}  messages={}  workspace={}\n",
            index + 1,
            id,
            status,
            messages,
            workspace
        ));
    }
    if sessions.len() > 20 {
        text.push_str(&format!("  ... {} more\n", sessions.len() - 20));
    }
    text
}

pub(super) fn remote_tasks_text(payload: &Value) -> String {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let tree = payload
        .get("tree")
        .and_then(Value::as_array)
        .or_else(|| payload.get("tasks").and_then(Value::as_array));
    let Some(tasks) = tree else {
        return format!("Remote tasks for {session_id}: none\n");
    };
    if tasks.is_empty() {
        return format!("Remote tasks for {session_id}: none\n");
    }
    let mut text = format!("Remote tasks for {session_id}:\n");
    for task in tasks {
        append_remote_task_text(&mut text, task, 1);
    }
    text
}

fn append_remote_task_text(text: &mut String, task: &Value, depth: usize) {
    let indent = "  ".repeat(depth);
    let id = task
        .get("session_id")
        .or_else(|| task.get("task_id"))
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let title = task
        .get("title")
        .or_else(|| task.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("task");
    let status = task
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let subagent = task
        .get("subagent_type")
        .or_else(|| task.get("agent"))
        .and_then(Value::as_str)
        .unwrap_or("subagent");
    let background = task
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let depth_value = task
        .get("task_depth")
        .and_then(Value::as_u64)
        .unwrap_or(depth as u64);
    let marker = if background { " background" } else { "" };
    text.push_str(&format!(
        "{indent}- {id}  [{status}] {subagent}  depth={depth_value}{marker}  {title}\n"
    ));
    if let Some(children) = task.get("children").and_then(Value::as_array) {
        for child in children {
            append_remote_task_text(text, child, depth + 1);
        }
    }
}

pub(super) fn tui_lines(
    kind: &str,
    text: impl Into<String>,
    important: bool,
) -> Vec<openagent_tui::TimelineLine> {
    let text = text.into();
    if text.trim().is_empty() {
        return Vec::new();
    }
    text.lines()
        .map(|line| openagent_tui::TimelineLine::new(kind, line.to_string(), important))
        .collect()
}
