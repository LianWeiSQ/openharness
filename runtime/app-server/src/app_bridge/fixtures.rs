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
