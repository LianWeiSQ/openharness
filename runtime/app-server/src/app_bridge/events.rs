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
