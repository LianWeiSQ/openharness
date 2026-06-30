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
