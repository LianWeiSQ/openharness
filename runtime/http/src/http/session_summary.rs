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
