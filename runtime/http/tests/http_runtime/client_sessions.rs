#[test]
fn remote_runtime_client_round_trips_tui_approval() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-client-approval")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(
        &session_id,
        "run approved command",
        serde_json::json!({
            "permission": "PLAN_ONLY",
            "tool_call": {
                "call_id": "call_bash",
                "name": "bash",
                "input": {"command": "printf approved"}
            }
        }),
    )?;
    assert_eq!(started["status"], "waiting_approval");
    let approval = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["method"] == "turn/approval_requested")
        .and_then(|event| event["params"]["approval"].as_object())
        .cloned()
        .expect("approval");
    let mut response = Value::Object(approval);
    response["action"] = Value::String("allow".to_string());
    response["scope"] = Value::String("once".to_string());

    let resolved = client.respond_approval(&response)?;
    let events = resolved["events"].as_array().expect("resolved events");

    assert!(events.iter().any(|event| {
        event["method"] == "item/toolCall/completed" && event["params"]["output"] == "approved"
    }));
    assert!(events.iter().any(|event| {
        event["method"] == "turn/completed" && event["params"]["status"] == "completed"
    }));

    let global_events = client.global_events(0)?;
    assert!(
        global_events
            .iter()
            .any(|event| event["method"] == "turn/approval_resolved")
    );

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_manages_session_lifecycle() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-session-lifecycle")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let renamed =
        client.update_session(&session_id, serde_json::json!({"title": "Alpha Session"}))?;
    assert_eq!(renamed["session"]["title"], "Alpha Session");

    let search = client.search_sessions("Alpha")?;
    assert_eq!(search.len(), 1);
    assert_eq!(search[0]["session_id"], session_id);

    let child_id = client.create_session(&workspace, Some(&session_id))?;
    let children = client.children(&session_id)?;
    assert!(
        children
            .iter()
            .any(|child| child["session_id"] == child_id && child["forked_from"] == session_id)
    );

    let share = client.share_session(&session_id)?;
    assert_eq!(share["shared"], true);
    assert!(
        share["url"]
            .as_str()
            .unwrap_or_default()
            .starts_with("openagent://share/")
    );
    let unshare = client.unshare_session(&session_id)?;
    assert_eq!(unshare["shared"], false);

    let compact = client.compact_session(&session_id)?;
    assert_eq!(compact["status"], "compacted");
    assert!(compact["summary"]["summary"].as_str().is_some());

    let archived = client.update_session(&session_id, serde_json::json!({"archived": true}))?;
    assert_eq!(archived["session"]["archived"], true);

    let deleted_child = client.delete_session(&child_id)?;
    assert_eq!(deleted_child["removed"], true);
    let deleted_parent = client.delete_session(&session_id)?;
    assert_eq!(deleted_parent["removed"], true);

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_reads_session_transcript() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-session-transcript")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(&session_id, "hello transcript", serde_json::json!({}))?;
    assert_eq!(started["status"], "completed");

    let transcript = client.session_messages(&session_id, Some(2))?;
    let messages = transcript["messages"].as_array().expect("messages");
    let messages_v2 = transcript["messages_v2"].as_array().expect("messages_v2");

    assert_eq!(transcript["session_id"], session_id);
    assert_eq!(transcript["message_count"], 2);
    assert_eq!(transcript["message_v2_count"], 2);
    assert_eq!(transcript["limit"], 2);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages_v2.len(), 2);
    assert_eq!(messages[0]["index"], 0);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "hello transcript");
    assert_eq!(messages_v2[0]["info"]["role"], "user");
    assert_eq!(messages_v2[0]["parts"][0]["kind"], "text");
    assert_eq!(messages_v2[0]["parts"][0]["content"], "hello transcript");
    assert_eq!(messages[1]["index"], 1);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages_v2[1]["info"]["role"], "assistant");
    assert_eq!(messages_v2[1]["parts"][0]["kind"], "text");
    assert!(
        !messages[1]["content"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .is_empty()
    );

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_tracks_file_diff_undo_and_redo() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-file-diff")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let file_path = workspace.join("notes.txt");

    let write = client.start_turn(
        &session_id,
        "write notes",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_write_notes",
                "name": "write",
                "input": {"file_path": "notes.txt", "content": "alpha\n"}
            }
        }),
    )?;
    assert_eq!(write["status"], "completed");
    assert_eq!(fs::read_to_string(&file_path)?, "alpha\n");

    let diff = client.session_diff(&session_id)?;
    assert_eq!(diff["undo_count"], 1);
    assert_eq!(diff["redo_count"], 0);
    assert!(
        diff["latest"]["diff"]
            .as_str()
            .unwrap_or_default()
            .contains("+alpha")
    );

    let undo = client.undo_session(&session_id)?;
    assert_eq!(undo["status"], "undone");
    assert!(!file_path.exists());
    assert_eq!(undo["redo_count"], 1);

    let redo = client.redo_session(&session_id)?;
    assert_eq!(redo["status"], "redone");
    assert_eq!(fs::read_to_string(&file_path)?, "alpha\n");
    assert_eq!(redo["undo_count"], 1);

    let edited = client.start_turn(
        &session_id,
        "edit notes",
        serde_json::json!({
            "permission": "FULL",
            "tool_calls": [
                {
                    "call_id": "call_read_notes",
                    "name": "read",
                    "input": {"file_path": "notes.txt"}
                },
                {
                    "call_id": "call_edit_notes",
                    "name": "edit",
                    "input": {
                        "file_path": "notes.txt",
                        "old_string": "alpha",
                        "new_string": "beta"
                    }
                }
            ]
        }),
    )?;
    assert_eq!(edited["status"], "completed");
    assert_eq!(fs::read_to_string(&file_path)?, "beta\n");
    assert_eq!(client.session_diff(&session_id)?["undo_count"], 2);

    let edit_undo = client.undo_session(&session_id)?;
    assert_eq!(edit_undo["status"], "undone");
    assert_eq!(fs::read_to_string(&file_path)?, "alpha\n");

    let edit_redo = client.redo_session(&session_id)?;
    assert_eq!(edit_redo["status"], "redone");
    assert_eq!(fs::read_to_string(&file_path)?, "beta\n");

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_controls_model_agent_variant_and_thinking() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-profile")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;

    let models = client.models()?;
    assert!(
        models["models"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(
        models["variants"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == "deep"))
    );

    let agents = client.agents()?;
    assert!(
        agents["agents"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["id"] == "coder"))
    );

    let updated = client.update_session(
        &session_id,
        serde_json::json!({
            "agent": "coder",
            "model": "server-local",
            "variant": "deep",
            "thinking": "high"
        }),
    )?;
    assert_eq!(updated["session"]["metadata"]["agent"], "coder");
    assert_eq!(updated["session"]["metadata"]["variant"], "deep");
    assert_eq!(updated["session"]["metadata"]["thinking"], "high");

    let started = client.start_turn(&session_id, "profile turn", serde_json::json!({}))?;
    let turn_started = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["method"] == "turn/started")
        .expect("turn started event");
    assert_eq!(turn_started["params"]["agent"], "coder");
    assert_eq!(turn_started["params"]["model"], "server-local");
    assert_eq!(turn_started["params"]["variant"], "deep");
    assert_eq!(turn_started["params"]["thinking"], "high");
    assert_eq!(started["turn"]["agent"], "coder");
    assert_eq!(started["turn"]["variant"], "deep");
    assert_eq!(started["turn"]["trace"]["agent"], "coder");
    assert!(
        started["turn"]["usage"]["total_tokens"]
            .as_u64()
            .is_some_and(|value| value > 0)
    );

    let override_started = client.start_turn(
        &session_id,
        "override profile",
        serde_json::json!({"agent": "reviewer", "variant": "fast", "thinking": "low"}),
    )?;
    let override_event = override_started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["method"] == "turn/started")
        .expect("turn started event");
    assert_eq!(override_event["params"]["agent"], "reviewer");
    assert_eq!(override_event["params"]["variant"], "fast");
    assert_eq!(override_event["params"]["thinking"], "low");

    let session = client.get_session(&session_id)?;
    assert_eq!(session["metadata"]["agent"], "reviewer");
    assert_eq!(session["metadata"]["variant"], "fast");

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}
