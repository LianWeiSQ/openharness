use super::common::*;

#[test]
fn file_session_store_writes_summary_and_restores_state() {
    let root = unique_temp_dir("openagent-session-store");
    let store = FileSessionStore::new(root.join("sessions"));
    let mut session = Session::new("session_test", root.join("workspace"));
    session.status = SessionStatus::Running;
    session.set_todos(vec![TodoItem::new(
        "port session store",
        "in_progress",
        "high",
        "todo-1",
    )]);

    let metadata = store
        .start_run(
            &mut session,
            StartRunOptions {
                run_id: "run_test".to_string(),
                trace_id: "trace_test".to_string(),
                agent_name: "agent".to_string(),
                model_id: Some("model".to_string()),
                provider_id: Some("provider".to_string()),
                permission: "FULL".to_string(),
                max_steps: 3,
                started_at_ms: Some(1_781_840_000_000),
            },
        )
        .expect("run starts");
    let message = ChatMessage {
        role: Role::User,
        content: "hello".to_string(),
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::from([("message_id".to_string(), json!("msg_1"))]),
    };
    session.add(message.clone());
    store
        .append_message(&session, &message, "run_test", 0)
        .expect("message appends");
    store
        .record_event(
            "session_test",
            "run_test",
            "model.usage",
            SessionEventOptions {
                kind: "model".to_string(),
                attributes: BTreeMap::from([
                    ("input_tokens".to_string(), json!(5)),
                    ("output_tokens".to_string(), json!(7)),
                    ("cost".to_string(), json!(0.02)),
                ]),
                ..SessionEventOptions::default()
            },
        )
        .expect("usage event records");
    store
        .append_part(
            "session_test",
            "run_test",
            "usage",
            SessionPartOptions {
                attributes: BTreeMap::from([
                    ("input_tokens".to_string(), json!(5)),
                    ("output_tokens".to_string(), json!(7)),
                ]),
                step_index: Some(1),
                ..SessionPartOptions::default()
            },
        )
        .expect("part appends");
    session.status = SessionStatus::Idle;
    store
        .finish_run(&session, "run_test", "completed", 1, Some("stop"), None)
        .expect("run finishes");

    let summary = read_json(root.join("sessions/session_test/runs/run_test/summary.json"));
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["message_count"], 1);
    assert_eq!(summary["part_type_counts"]["usage"], 1);
    assert_eq!(summary["total_input_tokens"], 5);
    assert_eq!(summary["total_output_tokens"], 7);

    let restored = store
        .load_session("session_test")
        .expect("session restores");
    assert_eq!(restored.id, "session_test");
    assert_eq!(restored.messages[0].content, "hello");
    assert_eq!(restored.todos[0].id, "todo-1");
    assert_eq!(
        restored.metadata["session_store"]["ledger_path"],
        json!(metadata.ledger_path)
    );
    let parts = store
        .load_parts("session_test", "run_test")
        .expect("parts load");
    assert_eq!(parts[0].part_type, "usage");

    fs::remove_dir_all(root).expect("temporary session store is removed");
}

#[test]
fn file_session_store_writes_message_v2_parts_and_attaches_tool_results() {
    let root = unique_temp_dir("openagent-message-v2-store");
    let store = FileSessionStore::new(root.join("sessions"));
    let mut session = Session::new("session_msg_v2", root.join("workspace"));
    store
        .start_run(
            &mut session,
            StartRunOptions {
                run_id: "run_msg_v2".to_string(),
                trace_id: "trace_msg_v2".to_string(),
                agent_name: "agent".to_string(),
                model_id: Some("model".to_string()),
                provider_id: Some("provider".to_string()),
                permission: "FULL".to_string(),
                max_steps: 3,
                started_at_ms: Some(1),
            },
        )
        .expect("run starts");

    let assistant = ChatMessage {
        role: Role::Assistant,
        content: "I'll read it.".to_string(),
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::from([
            ("message_id".to_string(), json!("msg_assistant")),
            ("step".to_string(), json!(1)),
            (
                "tool_calls".to_string(),
                json!([{
                    "id": "call_read",
                    "call_id": "call_read",
                    "type": "function",
                    "function": {"name": "read", "arguments": "{\"path\":\"Cargo.toml\"}"},
                    "name": "read",
                    "input": {"path": "Cargo.toml"}
                }]),
            ),
        ]),
    };
    session.add(assistant.clone());
    store
        .append_message(&session, &assistant, "run_msg_v2", 0)
        .expect("assistant appends");

    let tool = ChatMessage {
        role: Role::Tool,
        content: "[workspace]".to_string(),
        name: Some("read".to_string()),
        tool_call_id: Some("call_read".to_string()),
        metadata: BTreeMap::from([
            ("assistant_message_id".to_string(), json!("msg_assistant")),
            ("step".to_string(), json!(1)),
            (
                "tool_result".to_string(),
                json!({
                    "call_id": "call_read",
                    "output": "[workspace]",
                    "error": null,
                    "metadata": {}
                }),
            ),
        ]),
    };
    session.add(tool.clone());
    store
        .append_message(&session, &tool, "run_msg_v2", 1)
        .expect("tool appends");

    let messages = store
        .list_messages_with_parts("session_msg_v2", None, None)
        .expect("messages list");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].info.id, "msg_assistant");
    assert!(messages[0].parts.iter().any(|part| {
        part.kind == MessagePartKind::Tool && part.status == MessageStatus::Pending
    }));
    assert!(messages[0].parts.iter().any(|part| {
        part.kind == MessagePartKind::Tool && part.status == MessageStatus::Completed
    }));

    let projected = message_parts_to_chat_messages(&messages);
    assert_eq!(projected.len(), 2);
    assert_eq!(projected[0].role, Role::Assistant);
    assert_eq!(projected[1].role, Role::Tool);
    assert_eq!(projected[1].tool_call_id.as_deref(), Some("call_read"));
    assert_eq!(projected[1].content, "[workspace]");

    fs::remove_dir_all(root).expect("temporary session store is removed");
}

#[test]
fn file_session_store_projects_legacy_v1_transcripts_to_message_v2() {
    let root = unique_temp_dir("openagent-message-v2-legacy");
    let session_dir = root.join("sessions/session_legacy");
    fs::create_dir_all(&session_dir).expect("session dir exists");
    fs::write(
        session_dir.join("transcript.jsonl"),
        serde_json::to_string(&json!({
            "schema_version": "openagent.message.v1",
            "message_id": "msg_legacy",
            "session_id": "session_legacy",
            "run_id": "run_legacy",
            "index": 0,
            "role": "user",
            "content": "hello from v1",
            "name": null,
            "tool_call_id": null,
            "metadata": {},
            "timestamp_ms": 1,
        }))
        .expect("json serializes")
            + "\n",
    )
    .expect("legacy transcript writes");

    let store = FileSessionStore::new(root.join("sessions"));
    let messages = store
        .list_messages_with_parts("session_legacy", None, None)
        .expect("legacy projects");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].info.id, "msg_legacy");
    assert_eq!(messages[0].parts[0].kind, MessagePartKind::Text);
    assert_eq!(messages[0].parts[0].content, json!("hello from v1"));

    fs::remove_dir_all(root).expect("temporary session store is removed");
}
