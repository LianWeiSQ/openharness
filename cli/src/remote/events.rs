use super::*;

fn remote_turn_events(
    server_url: &str,
    auth: &RemoteAuth,
    turn_id: &str,
    last_event_id: u64,
) -> Result<Vec<Value>, String> {
    let path = if last_event_id > 0 {
        format!("/api/turns/{turn_id}/events?last_event_id={last_event_id}")
    } else {
        format!("/api/turns/{turn_id}/events")
    };
    let raw = http_text_with_auth("GET", server_url, &path, auth, None)?;
    openagent_http_runtime::parse_sse_response_lines(&raw.lines().collect::<Vec<_>>())
}

pub(crate) fn remote_events_for_payload(
    server_url: &str,
    auth: &RemoteAuth,
    payload: &Value,
) -> Result<Vec<Value>, String> {
    let fallback = payload
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(turn_id) = remote_turn_id(payload) else {
        return Ok(fallback);
    };
    let mut events = Vec::new();
    let mut seen = BTreeSet::new();
    let mut last_id = 0_u64;
    for event in fallback {
        last_id = last_id.max(app_event_sequence(&event));
        if let Some(key) = app_event_dedupe_key(&event)
            && !seen.insert(key)
        {
            continue;
        }
        events.push(event);
    }
    let deadline = SystemTime::now() + Duration::from_secs(remote_attach_wait_seconds());
    loop {
        match remote_turn_events(server_url, auth, &turn_id, last_id) {
            Ok(next) => {
                let mut advanced = false;
                for event in next {
                    let seq = app_event_sequence(&event);
                    if seq > last_id {
                        last_id = seq;
                    }
                    if let Some(key) = app_event_dedupe_key(&event)
                        && !seen.insert(key)
                    {
                        continue;
                    }
                    advanced = true;
                    events.push(event);
                }
                if events.iter().any(is_terminal_app_event) {
                    return Ok(events);
                }
                if !advanced && SystemTime::now() >= deadline {
                    return Ok(events);
                }
            }
            Err(error) if events.is_empty() => return Err(error),
            Err(_) => return Ok(events),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn remote_attach_wait_seconds() -> u64 {
    env::var("OPENAGENT_ATTACH_WAIT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30)
}

pub(super) fn app_event_sequence(event: &Value) -> u64 {
    event
        .get("sequence")
        .or_else(|| event.get("global_sequence"))
        .and_then(Value::as_u64)
        .unwrap_or_default()
}

pub(super) fn app_event_dedupe_key(event: &Value) -> Option<String> {
    Some(format!(
        "{}:{}:{}",
        app_event_sequence(event),
        event.get("method").and_then(Value::as_str).unwrap_or(""),
        stable_json_dumps(event.get("params").unwrap_or(&Value::Null))
    ))
}

fn is_terminal_app_event(event: &Value) -> bool {
    matches!(
        event.get("method").and_then(Value::as_str),
        Some("turn/completed" | "turn/failed" | "turn/interrupted")
    ) || matches!(
        event
            .get("params")
            .and_then(|params| params.get("status"))
            .and_then(Value::as_str),
        Some("completed" | "failed" | "interrupted")
    )
}

pub(super) fn remote_turn_id(payload: &Value) -> Option<String> {
    payload
        .get("turn_id")
        .or_else(|| payload.get("turn").and_then(|turn| turn.get("id")))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn text_from_app_events(events: &[Value]) -> String {
    let mut text = String::new();
    let mut final_answer = String::new();
    for event in events {
        let method = event
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = event.get("params").unwrap_or(&Value::Null);
        if method == "item/agentMessage/delta" {
            if let Some(delta) = params
                .get("delta")
                .or_else(|| params.get("text"))
                .or_else(|| params.get("event").and_then(|event| event.get("text")))
                .and_then(Value::as_str)
            {
                text.push_str(delta);
            }
        }
        if method == "turn/approval_requested" {
            let tool = params
                .get("approval")
                .and_then(|approval| approval.get("tool_name"))
                .and_then(Value::as_str)
                .unwrap_or("tool");
            text.push_str(&format!("\n[approval required: {tool}]\n"));
        }
        if method == "turn/interrupted" {
            text.push_str("\n[turn interrupted]\n");
        }
        if method == "turn/completed" {
            final_answer = params
                .get("final_answer")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
    }
    if text.is_empty() { final_answer } else { text }
}
