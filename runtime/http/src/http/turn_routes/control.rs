fn interrupt_turn_payload(config: &HttpRuntimeConfig, turn_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let (session_id, mut session) = find_session_for_turn(&store, turn_id)?;
    session.status = SessionStatus::Stop;
    let _ = store.finish_run(
        &session,
        turn_id,
        "failed",
        1,
        Some("interrupted"),
        Some("interrupt requested"),
    );
    let event = json!({
        "method": "turn/interrupted",
        "params": {
            "session_id": session_id,
            "thread_id": session_id,
            "turn_id": turn_id,
            "status": "interrupted",
            "error": "interrupt requested",
        }
    });
    append_app_events(
        &store.root,
        &session_id,
        turn_id,
        std::slice::from_ref(&event),
    );
    Ok(json!({
        "session_id": session_id,
        "turn_id": turn_id,
        "status": "interrupted",
        "events": [event],
    }))
}

fn enqueue_tui_control_payload(
    config: &HttpRuntimeConfig,
    path: &str,
    body: &str,
) -> Result<Value, String> {
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let request = tui_control_request_for_path(path, &payload)?;
    let mut queue = read_json_array(&tui_control_queue_path(config));
    queue.push(request.to_value());
    write_json_value(&tui_control_queue_path(config), &Value::Array(queue))?;
    Ok(json!({"queued": true, "request": request.to_value()}))
}

fn pop_tui_control_payload(config: &HttpRuntimeConfig) -> Value {
    let path = tui_control_queue_path(config);
    let mut queue = read_json_array(&path);
    if queue.is_empty() {
        return control_next_payload(None);
    }
    let next = queue.remove(0);
    let _ = write_json_value(&path, &Value::Array(queue));
    let request = next.as_object().map(|_| {
        openagent_app_server::TuiControlRequest::new(
            next.get("path").and_then(Value::as_str).unwrap_or_default(),
            next.get("body").cloned().unwrap_or(Value::Null),
        )
    });
    control_next_payload(request.as_ref())
}

fn record_tui_control_response(config: &HttpRuntimeConfig, body: &str) -> Value {
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let response = record_control_response_payload(payload);
    append_json_line(&tui_control_responses_path(config), &response);
    response
}
