#[test]
fn app_bridge_terminal_keyflow_smoke_uses_real_remote_handler() -> Result<(), Box<dyn Error>> {
    let bridge = FakeAppBridge::start()?;
    let workspace = temp_test_dir("openagent-tui-bridge-keyflow")?;
    fs::write(workspace.join("notes.txt"), "hello from workspace\n")?;
    let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
        server_url: bridge.server_url.clone(),
        auth: RemoteAuth::bearer("secret"),
        workspace: workspace.clone(),
        ..AppBridgeTerminalOptions::default()
    })
    .map_err(std::io::Error::other)?;
    let mut state = TuiState::new();

    let initial = handler.initial_lines();
    apply_handler_output(&mut state, &mut handler, initial);
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("connected to"))
    );

    send_key_text("/new", &mut state, &mut handler)?;
    press_key(KeyCode::Enter, &mut state, &mut handler)?;
    assert_eq!(handler.current_session(), Some("session_smoke"));
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("created session: session_smoke"))
    );

    send_key_text("hello bridge", &mut state, &mut handler)?;
    press_key(KeyCode::Enter, &mut state, &mut handler)?;
    assert_eq!(state.session_id.as_deref(), Some("session_smoke"));
    assert_eq!(state.current_turn_id.as_deref(), Some("turn_smoke"));
    assert_eq!(state.status, "completed");
    assert_eq!(state.usage_totals["total_tokens"], json!(3));
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.kind == "assistant" && line.text.contains("bridge answer"))
    );

    let polled = handler.poll_app_events().map_err(std::io::Error::other)?;
    apply_app_event_values(&mut state, polled);
    assert!(
        state
            .runtime_warnings
            .iter()
            .any(|warning| warning.contains("bridge smoke warning"))
    );

    let recorded = bridge.requests();
    assert!(recorded.iter().any(|request| request == "GET /api/health"));
    assert!(
        recorded
            .iter()
            .any(|request| request == "GET /api/sessions")
    );
    assert!(
        recorded
            .iter()
            .any(|request| request == "POST /api/sessions")
    );
    assert!(
        recorded
            .iter()
            .any(|request| request == "POST /api/sessions/session_smoke/turns")
    );
    assert!(
        recorded
            .iter()
            .any(|request| request.starts_with("GET /api/events?last_event_id="))
    );
    assert!(
        bridge
            .turn_inputs()
            .iter()
            .any(|input| input == "hello bridge")
    );

    bridge.stop();
    let _ = fs::remove_dir_all(workspace);
    Ok(())
}

#[test]
fn app_bridge_terminal_transcript_reads_real_session_messages() -> Result<(), Box<dyn Error>> {
    let bridge = FakeAppBridge::start()?;
    let workspace = temp_test_dir("openagent-tui-bridge-transcript")?;
    let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
        server_url: bridge.server_url.clone(),
        auth: RemoteAuth::bearer("secret"),
        workspace: workspace.clone(),
        session_id: Some("session_smoke".to_string()),
        ..AppBridgeTerminalOptions::default()
    })
    .map_err(std::io::Error::other)?;

    let lines = handler
        .handle_command("/transcript 2")
        .map_err(std::io::Error::other)?;

    assert!(
        lines
            .iter()
            .any(|line| line.text.contains("transcript: 2 of 3 message(s)"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.kind == "message" && line.text.contains("#1 assistant"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.kind == "message" && line.text.contains("bridge answer"))
    );
    let recorded = bridge.requests();
    assert!(
        recorded
            .iter()
            .any(|request| { request == "GET /api/sessions/session_smoke/messages?limit=2" })
    );

    bridge.stop();
    let _ = fs::remove_dir_all(workspace);
    Ok(())
}

#[test]
fn app_bridge_terminal_session_picker_searches_and_resumes() -> Result<(), Box<dyn Error>> {
    let bridge = FakeAppBridge::start()?;
    let workspace = temp_test_dir("openagent-tui-bridge-session-picker")?;
    let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
        server_url: bridge.server_url.clone(),
        auth: RemoteAuth::bearer("secret"),
        workspace: workspace.clone(),
        ..AppBridgeTerminalOptions::default()
    })
    .map_err(std::io::Error::other)?;
    let mut state = TuiState::new();

    send_key_text("/sessions smoke", &mut state, &mut handler)?;
    press_key(KeyCode::Enter, &mut state, &mut handler)?;

    assert_eq!(
        state
            .session_picker
            .as_ref()
            .expect("session picker")
            .candidates
            .len(),
        1
    );
    assert!(
        state
            .session_picker
            .as_ref()
            .expect("session picker")
            .candidates[0]["session_id"]
            == json!("session_smoke")
    );

    press_key(KeyCode::Enter, &mut state, &mut handler)?;

    assert!(state.session_picker.is_none());
    assert_eq!(state.session_id.as_deref(), Some("session_smoke"));
    assert_eq!(handler.current_session(), Some("session_smoke"));
    let recorded = bridge.requests();
    assert!(
        recorded
            .iter()
            .any(|request| request == "GET /api/sessions?query=smoke")
    );

    bridge.stop();
    let _ = fs::remove_dir_all(workspace);
    Ok(())
}

#[test]
fn app_bridge_terminal_session_picker_manages_real_session_actions() -> Result<(), Box<dyn Error>> {
    let bridge = FakeAppBridge::start()?;
    let workspace = temp_test_dir("openagent-tui-bridge-session-actions")?;
    let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
        server_url: bridge.server_url.clone(),
        auth: RemoteAuth::bearer("secret"),
        workspace: workspace.clone(),
        ..AppBridgeTerminalOptions::default()
    })
    .map_err(std::io::Error::other)?;
    let mut state = TuiState::new();

    send_key_text("/sessions smoke", &mut state, &mut handler)?;
    press_key(KeyCode::Enter, &mut state, &mut handler)?;
    press_key(KeyCode::Right, &mut state, &mut handler)?;
    press_key(KeyCode::Down, &mut state, &mut handler)?;
    press_key(KeyCode::Down, &mut state, &mut handler)?;
    press_key(KeyCode::Enter, &mut state, &mut handler)?;
    for _ in 0.."Smoke Session".len() {
        press_key(KeyCode::Backspace, &mut state, &mut handler)?;
    }
    send_key_text("Smoke Renamed", &mut state, &mut handler)?;
    press_key(KeyCode::Enter, &mut state, &mut handler)?;

    press_key(KeyCode::Right, &mut state, &mut handler)?;
    for _ in 0..3 {
        press_key(KeyCode::Down, &mut state, &mut handler)?;
    }
    press_key(KeyCode::Enter, &mut state, &mut handler)?;
    assert!(matches!(
        state.session_picker.as_ref().expect("session picker").mode,
        SessionPickerMode::Confirm(SessionPickerAction::Archive)
    ));
    press_key(KeyCode::Enter, &mut state, &mut handler)?;

    let updates = bridge.session_update_payloads();
    assert!(
        updates
            .iter()
            .any(|payload| payload["title"] == json!("Smoke Renamed")),
        "expected rename PATCH payload, got {updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|payload| payload["archived"] == json!(true)),
        "expected archive PATCH payload, got {updates:?}"
    );
    let recorded = bridge.requests();
    assert!(
        recorded
            .iter()
            .filter(|request| request.as_str() == "PATCH /api/sessions/session_smoke")
            .count()
            >= 2
    );

    bridge.stop();
    let _ = fs::remove_dir_all(workspace);
    Ok(())
}

#[test]
fn app_bridge_terminal_model_picker_fetches_and_sets_model() -> Result<(), Box<dyn Error>> {
    let bridge = FakeAppBridge::start()?;
    let workspace = temp_test_dir("openagent-tui-bridge-model-picker")?;
    let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
        server_url: bridge.server_url.clone(),
        auth: RemoteAuth::bearer("secret"),
        workspace: workspace.clone(),
        session_id: Some("session_smoke".to_string()),
        ..AppBridgeTerminalOptions::default()
    })
    .map_err(std::io::Error::other)?;
    let mut state = TuiState::new();

    send_key_text("/models", &mut state, &mut handler)?;
    press_key(KeyCode::Enter, &mut state, &mut handler)?;

    assert_eq!(
        state
            .model_picker
            .as_ref()
            .expect("model picker")
            .candidates
            .len(),
        2
    );

    press_key(KeyCode::Char('d'), &mut state, &mut handler)?;
    assert_eq!(
        state
            .model_picker
            .as_ref()
            .expect("model picker")
            .candidates[0]["id"],
        json!("deep-model")
    );

    press_key(KeyCode::Enter, &mut state, &mut handler)?;

    assert!(state.model_picker.is_none());
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("model set to deep-model"))
    );
    let model_updates = bridge.model_update_payloads();
    assert_eq!(model_updates.len(), 1);
    assert_eq!(model_updates[0]["model"], json!("deep-model"));
    let recorded = bridge.requests();
    assert!(recorded.iter().any(|request| request == "GET /api/models"));
    assert!(
        recorded
            .iter()
            .any(|request| request == "PATCH /api/sessions/session_smoke")
    );

    bridge.stop();
    let _ = fs::remove_dir_all(workspace);
    Ok(())
}

#[test]
fn app_bridge_terminal_agent_picker_fetches_and_sets_agent() -> Result<(), Box<dyn Error>> {
    let bridge = FakeAppBridge::start()?;
    let workspace = temp_test_dir("openagent-tui-bridge-agent-picker")?;
    let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
        server_url: bridge.server_url.clone(),
        auth: RemoteAuth::bearer("secret"),
        workspace: workspace.clone(),
        session_id: Some("session_smoke".to_string()),
        ..AppBridgeTerminalOptions::default()
    })
    .map_err(std::io::Error::other)?;
    let mut state = TuiState::new();

    send_key_text("/agents", &mut state, &mut handler)?;
    press_key(KeyCode::Enter, &mut state, &mut handler)?;

    assert_eq!(
        state
            .agent_picker
            .as_ref()
            .expect("agent picker")
            .candidates
            .len(),
        2
    );

    send_key_text("rev", &mut state, &mut handler)?;
    assert_eq!(
        state
            .agent_picker
            .as_ref()
            .expect("agent picker")
            .candidates[0]["id"],
        json!("reviewer")
    );

    press_key(KeyCode::Enter, &mut state, &mut handler)?;

    assert!(state.agent_picker.is_none());
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("agent set to reviewer"))
    );
    let agent_updates = bridge.agent_update_payloads();
    assert_eq!(agent_updates.len(), 1);
    assert_eq!(agent_updates[0]["agent"], json!("reviewer"));
    let recorded = bridge.requests();
    assert!(recorded.iter().any(|request| request == "GET /api/agents"));
    assert!(
        recorded
            .iter()
            .any(|request| request == "PATCH /api/sessions/session_smoke")
    );

    bridge.stop();
    let _ = fs::remove_dir_all(workspace);
    Ok(())
}

#[test]
fn app_bridge_terminal_variant_and_thinking_pickers_fetch_and_set() -> Result<(), Box<dyn Error>> {
    let bridge = FakeAppBridge::start()?;
    let workspace = temp_test_dir("openagent-tui-bridge-choice-picker")?;
    let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
        server_url: bridge.server_url.clone(),
        auth: RemoteAuth::bearer("secret"),
        workspace: workspace.clone(),
        session_id: Some("session_smoke".to_string()),
        ..AppBridgeTerminalOptions::default()
    })
    .map_err(std::io::Error::other)?;
    let mut state = TuiState::new();

    send_key_text("/variant", &mut state, &mut handler)?;
    press_key(KeyCode::Enter, &mut state, &mut handler)?;
    send_key_text("dee", &mut state, &mut handler)?;
    assert_eq!(
        state
            .choice_picker
            .as_ref()
            .expect("variant picker")
            .candidates,
        vec!["deep".to_string()]
    );
    press_key(KeyCode::Enter, &mut state, &mut handler)?;

    assert!(state.choice_picker.is_none());
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("variant set to deep"))
    );
    let variant_updates = bridge.variant_update_payloads();
    assert_eq!(variant_updates.len(), 1);
    assert_eq!(variant_updates[0]["variant"], json!("deep"));

    send_key_text("/thinking", &mut state, &mut handler)?;
    press_key(KeyCode::Enter, &mut state, &mut handler)?;
    send_key_text("hi", &mut state, &mut handler)?;
    assert_eq!(
        state
            .choice_picker
            .as_ref()
            .expect("thinking picker")
            .candidates,
        vec!["high".to_string()]
    );
    press_key(KeyCode::Enter, &mut state, &mut handler)?;

    assert!(state.choice_picker.is_none());
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("thinking set to high"))
    );
    let thinking_updates = bridge.thinking_update_payloads();
    assert_eq!(thinking_updates.len(), 1);
    assert_eq!(thinking_updates[0]["thinking"], json!("high"));
    let recorded = bridge.requests();
    assert_eq!(
        recorded
            .iter()
            .filter(|request| request.as_str() == "GET /api/models")
            .count(),
        2
    );
    assert!(
        recorded
            .iter()
            .any(|request| request == "PATCH /api/sessions/session_smoke")
    );

    bridge.stop();
    let _ = fs::remove_dir_all(workspace);
    Ok(())
}

#[test]
fn app_bridge_terminal_interaction_keyflow_posts_real_responses() -> Result<(), Box<dyn Error>> {
    let bridge = FakeAppBridge::start()?;
    let workspace = temp_test_dir("openagent-tui-bridge-interactions")?;
    let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
        server_url: bridge.server_url.clone(),
        auth: RemoteAuth::bearer("secret"),
        workspace: workspace.clone(),
        ..AppBridgeTerminalOptions::default()
    })
    .map_err(std::io::Error::other)?;
    let mut state = TuiState::new();

    state.apply_app_event(&json!({
        "method": "turn/approval_requested",
        "params": {
            "session_id": "session_smoke",
            "thread_id": "session_smoke",
            "turn_id": "turn_approval",
            "status": "waiting_approval",
            "approval": {
                "request_id": "approval_smoke",
                "turn_id": "turn_approval",
                "session_id": "session_smoke",
                "tool_name": "bash",
                "tool_input": {"command": "printf ok"}
            }
        }
    }));
    press_key(KeyCode::Char('1'), &mut state, &mut handler)?;

    assert!(state.active_approval.is_none());
    assert_eq!(state.status, "completed");
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("approved through bridge"))
    );
    let approvals = bridge.approval_payloads();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0]["action"], json!("allow"));
    assert_eq!(approvals[0]["scope"], json!("once"));
    assert_eq!(approvals[0]["request_id"], json!("approval_smoke"));

    state.apply_app_event(&json!({
        "method": "item/question/requested",
        "params": {
            "session_id": "session_smoke",
            "thread_id": "session_smoke",
            "turn_id": "turn_question",
            "event": {
                "request_id": "question_smoke",
                "turn_id": "turn_question",
                "session_id": "session_smoke",
                "questions": [{
                    "header": "Mode",
                    "question": "Which path?",
                    "options": [
                        {"label": "Fast"},
                        {"label": "Safe"}
                    ]
                }]
            }
        }
    }));
    press_key(KeyCode::Char('2'), &mut state, &mut handler)?;

    assert!(state.active_question.is_none());
    assert_eq!(state.status, "completed");
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("question bridge answer"))
    );
    let questions = bridge.question_payloads();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0]["request_id"], json!("question_smoke"));
    assert_eq!(questions[0]["answers"], json!([["Safe"]]));

    let recorded = bridge.requests();
    assert!(
        recorded
            .iter()
            .any(|request| request == "POST /api/turns/turn_approval/approvals/approval_smoke")
    );
    assert!(recorded.iter().any(|request| {
        request == "POST /api/turns/turn_question/questions/question_smoke/reply"
    }));

    bridge.stop();
    let _ = fs::remove_dir_all(workspace);
    Ok(())
}
