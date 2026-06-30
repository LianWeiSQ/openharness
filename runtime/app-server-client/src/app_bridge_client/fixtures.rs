#[must_use]
pub fn app_bridge_client_fixture() -> Value {
    let parsed_event = app_event_from_value(
        &json!({
            "sequence": 4,
            "global_sequence": 12,
            "method": "turn/completed",
            "params": {
                "thread_id": "session_existing",
                "turn_id": "turn_remote",
                "status": "completed",
                "final_answer": "hello remote",
                "trace": {"id": "trace_1"},
            },
            "created_at_ms": 1781842000304u64,
        }),
        99,
    )
    .unwrap_or_else(|_| AppEvent {
        sequence: 0,
        method: String::new(),
        params: json!({}),
        created_at_ms: 0,
        global_sequence: None,
    });
    let key = remote_event_key(&parsed_event, "turn_remote");
    let mut remote_turn = RemoteTurnRecord::new("turn_remote", "session_existing");
    let remote_events = vec![
        AppEvent {
            sequence: 1,
            global_sequence: Some(10),
            method: "turn/started".to_string(),
            params: json!({"thread_id": "session_existing", "turn_id": "turn_remote", "status": "running"}),
            created_at_ms: 1_781_842_000_301,
        },
        AppEvent {
            sequence: 2,
            global_sequence: Some(11),
            method: "turn/approval_requested".to_string(),
            params: json!({
                "thread_id": "session_existing",
                "turn_id": "turn_remote",
                "status": "waiting_approval",
                "approval": {"turn_id": "turn_remote", "request_id": "approval_1", "tool_name": "write"},
            }),
            created_at_ms: 1_781_842_000_302,
        },
        AppEvent {
            sequence: 3,
            global_sequence: None,
            method: "turn/approval_resolved".to_string(),
            params: json!({
                "thread_id": "session_existing",
                "turn_id": "turn_remote",
                "status": "running",
                "approval": {"turn_id": "turn_remote", "request_id": "approval_1", "action": "deny"},
            }),
            created_at_ms: 1_781_842_000_303,
        },
        parsed_event.clone(),
    ];
    let append_results = remote_events
        .into_iter()
        .map(|event| remote_turn.append_event(event))
        .collect::<Vec<_>>();
    let duplicate_result = remote_turn.append_event(AppEvent {
        sequence: 1,
        global_sequence: Some(10),
        method: "turn/started".to_string(),
        params: json!({"thread_id": "session_existing", "turn_id": "turn_remote", "status": "running"}),
        created_at_ms: 1_781_842_000_301,
    });
    let events = remote_turn
        .events
        .iter()
        .map(AppEvent::to_value)
        .collect::<Vec<_>>();

    json!({
        "helpers": {
            "normalize": normalize_server_url("http://127.0.0.1:8787/"),
            "join": join_server_url("http://127.0.0.1:8787/", "/api/sessions"),
            "quote": quote_path("turn/a b"),
            "auth_header": auth_header(Some("secret")).unwrap_or_default(),
        },
        "parsed_event": parsed_event.to_value(),
        "event_ids": {
            "turn": event_turn_id(&parsed_event),
            "session": event_session_id(&parsed_event),
            "key": remote_event_key_value(&key),
        },
        "remote_turn": {
            "append_results": append_results,
            "duplicate_result": duplicate_result,
            "status": remote_turn.status,
            "final_answer": remote_turn.final_answer,
            "trace": remote_turn.trace,
            "events": events,
        },
        "request_shapes": {
            "start_session": request_shape("POST", "/api/sessions", Some(json!({"cwd": "/tmp/openagent-rust-rewrite-fixture-goal11/workspace"}))),
            "start_turn": request_shape("POST", "/api/sessions/session_existing/turns", Some(json!({"input": "hello"}))),
            "interrupt": request_shape("POST", "/api/turns/turn_remote/interrupt", Some(json!({}))),
            "approval": request_shape("POST", "/api/turns/turn_remote/approvals/approval_1", Some(json!({"action": "deny"}))),
            "control_next": request_shape("GET", "/tui/control/next?timeout=0.25", None),
            "control_response": request_shape("POST", "/tui/control/response", Some(json!({"ok": true, "result": {"applied": true}}))),
        },
    })
}
