#[test]
fn remote_control_select_model_dispatches_handler_command() {
    #[derive(Default)]
    struct CaptureHandler {
        commands: Vec<String>,
        responses: Vec<Value>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
            self.commands.push(command.to_string());
            Ok(vec![TimelineLine::new(
                "status",
                "command dispatched",
                true,
            )])
        }

        fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
            self.responses.push(payload.clone());
            Ok(())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    handle_remote_control_request(
        &mut state,
        &mut handler,
        &json!({"path": "/tui/select-model", "body": {"model": "gpt-test"}}),
    );

    assert_eq!(handler.commands, vec!["/models gpt-test".to_string()]);
    assert_eq!(handler.responses.len(), 1);
    assert_eq!(handler.responses[0]["ok"], json!(true));
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("command dispatched"))
    );
}

#[test]
fn remote_control_open_models_dispatches_picker_fetch() {
    #[derive(Default)]
    struct CaptureHandler {
        model_fetches: usize,
        responses: Vec<Value>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn list_models(&mut self) -> Result<Value, String> {
            self.model_fetches += 1;
            Ok(json!({
                "models": [
                    {"id": "server-local", "provider_id": "openagent", "name": "Server Local"},
                    {"id": "deep-model", "provider_id": "openagent", "name": "Deep Model"}
                ]
            }))
        }

        fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
            self.responses.push(payload.clone());
            Ok(())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    handle_remote_control_request(
        &mut state,
        &mut handler,
        &json!({"path": "/tui/open-models", "body": {}}),
    );

    assert_eq!(handler.model_fetches, 1);
    assert_eq!(
        state
            .model_picker
            .as_ref()
            .expect("model picker")
            .candidates
            .len(),
        2
    );
    assert_eq!(handler.responses.len(), 1);
    assert_eq!(handler.responses[0]["ok"], json!(true));
}

#[test]
fn remote_control_open_agents_dispatches_picker_fetch() {
    #[derive(Default)]
    struct CaptureHandler {
        agent_fetches: usize,
        responses: Vec<Value>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn list_agents(&mut self) -> Result<Value, String> {
            self.agent_fetches += 1;
            Ok(json!({
                "agents": [
                    {"id": "server", "name": "Server", "description": "Default server agent"},
                    {"id": "reviewer", "name": "Reviewer", "description": "Review code"}
                ]
            }))
        }

        fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
            self.responses.push(payload.clone());
            Ok(())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    handle_remote_control_request(
        &mut state,
        &mut handler,
        &json!({"path": "/tui/open-agents", "body": {}}),
    );

    assert_eq!(handler.agent_fetches, 1);
    assert_eq!(
        state
            .agent_picker
            .as_ref()
            .expect("agent picker")
            .candidates
            .len(),
        2
    );
    assert_eq!(handler.responses.len(), 1);
    assert_eq!(handler.responses[0]["ok"], json!(true));
}

#[test]
fn remote_control_open_variant_and_thinking_dispatch_picker_fetch() {
    #[derive(Default)]
    struct CaptureHandler {
        model_fetches: usize,
        responses: Vec<Value>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn list_models(&mut self) -> Result<Value, String> {
            self.model_fetches += 1;
            Ok(json!({
                "models": [],
                "variants": ["default", "deep"],
                "thinking": ["low", "high"]
            }))
        }

        fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
            self.responses.push(payload.clone());
            Ok(())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    handle_remote_control_request(
        &mut state,
        &mut handler,
        &json!({"path": "/tui/open-variants", "body": {}}),
    );

    assert_eq!(handler.model_fetches, 1);
    let picker = state.choice_picker.as_ref().expect("variant picker");
    assert_eq!(picker.kind, ChoicePickerKind::Variant);
    assert_eq!(
        picker.candidates,
        vec!["default".to_string(), "deep".to_string()]
    );
    assert_eq!(handler.responses.len(), 1);
    assert_eq!(handler.responses[0]["ok"], json!(true));

    handle_remote_control_request(
        &mut state,
        &mut handler,
        &json!({"path": "/tui/open-thinking", "body": {}}),
    );

    assert_eq!(handler.model_fetches, 2);
    let picker = state.choice_picker.as_ref().expect("thinking picker");
    assert_eq!(picker.kind, ChoicePickerKind::Thinking);
    assert_eq!(
        picker.candidates,
        vec!["low".to_string(), "high".to_string()]
    );
    assert_eq!(handler.responses.len(), 2);
    assert_eq!(handler.responses[1]["ok"], json!(true));
}

#[test]
fn remote_control_agent_variant_and_thinking_dispatch_handler_commands() {
    #[derive(Default)]
    struct CaptureHandler {
        commands: Vec<String>,
        responses: Vec<Value>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
            self.commands.push(command.to_string());
            Ok(vec![TimelineLine::new(
                "status",
                format!("handled {command}"),
                true,
            )])
        }

        fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
            self.responses.push(payload.clone());
            Ok(())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    for request in [
        json!({"path": "/tui/select-agent", "body": {"agent": "reviewer"}}),
        json!({"path": "/tui/select-variant", "body": {"variant": "deep"}}),
        json!({"path": "/tui/select-thinking", "body": {"level": "high"}}),
        json!({"path": "/tui/publish", "body": {"type": "tui.agent.select", "properties": {"id": "server"}}}),
        json!({"path": "/tui/publish", "body": {"type": "tui.variant.select", "properties": {"value": "fast"}}}),
        json!({"path": "/tui/publish", "body": {"type": "tui.thinking.select", "properties": {"value": "low"}}}),
    ] {
        handle_remote_control_request(&mut state, &mut handler, &request);
    }

    assert_eq!(
        handler.commands,
        vec![
            "/agent reviewer",
            "/variant deep",
            "/thinking high",
            "/agent server",
            "/variant fast",
            "/thinking low",
        ]
    );
    assert_eq!(handler.responses.len(), handler.commands.len());
    assert!(
        handler
            .responses
            .iter()
            .all(|payload| payload["ok"] == json!(true))
    );
}

#[test]
fn remote_control_file_picker_dispatches_and_selects_into_composer() {
    #[derive(Default)]
    struct CaptureHandler {
        commands: Vec<String>,
        responses: Vec<Value>,
        searches: Vec<String>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
            self.commands.push(command.to_string());
            Ok(vec![TimelineLine::new(
                "status",
                format!("handled {command}"),
                true,
            )])
        }

        fn search_files(&mut self, query: &str) -> Result<Vec<ComposerFileCandidate>, String> {
            self.searches.push(query.to_string());
            Ok(vec![
                ComposerFileCandidate {
                    reference: "@src/core.rs".to_string(),
                    kind: "file".to_string(),
                },
                ComposerFileCandidate {
                    reference: "@src/main.rs".to_string(),
                    kind: "file".to_string(),
                },
            ])
        }

        fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
            self.responses.push(payload.clone());
            Ok(())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    handle_remote_control_request(
        &mut state,
        &mut handler,
        &json!({"path": "/tui/open-files", "body": {"query": "main"}}),
    );
    assert!(handler.commands.is_empty());
    assert_eq!(handler.searches, vec!["main".to_string()]);
    assert_eq!(
        state
            .file_picker
            .as_ref()
            .expect("file picker")
            .candidates
            .len(),
        2
    );

    state.input_buffer = "review".to_string();
    handle_remote_control_request(
        &mut state,
        &mut handler,
        &json!({"path": "/tui/publish", "body": {"type": "tui.file.select", "properties": {"path": "src/main.rs", "line": 7}}}),
    );

    assert!(handler.commands.is_empty());
    assert!(state.file_picker.is_none());
    assert_eq!(state.input_buffer, "review @src/main.rs:7 ");
    assert_eq!(handler.responses.len(), 2);
    assert!(
        handler
            .responses
            .iter()
            .all(|payload| payload["ok"] == json!(true))
    );
}

#[test]
fn remote_control_open_sessions_dispatches_picker_search() {
    #[derive(Default)]
    struct CaptureHandler {
        searches: Vec<String>,
        responses: Vec<Value>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn search_sessions(&mut self, query: &str) -> Result<Vec<Value>, String> {
            self.searches.push(query.to_string());
            Ok(vec![json!({
                "session_id": "session_alpha",
                "title": "Alpha",
                "status": "idle",
                "message_count": 2,
                "workspace": "/tmp/work"
            })])
        }

        fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
            self.responses.push(payload.clone());
            Ok(())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    handle_remote_control_request(
        &mut state,
        &mut handler,
        &json!({"path": "/tui/open-sessions", "body": {"query": "alp"}}),
    );

    assert_eq!(handler.searches, vec!["alp".to_string()]);
    assert_eq!(
        state
            .session_picker
            .as_ref()
            .expect("session picker")
            .candidates
            .len(),
        1
    );
    assert_eq!(handler.responses.len(), 1);
    assert_eq!(handler.responses[0]["ok"], json!(true));
}

#[test]
fn remote_control_session_actions_dispatch_handler_commands() {
    #[derive(Default)]
    struct CaptureHandler {
        commands: Vec<String>,
        responses: Vec<Value>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
            self.commands.push(command.to_string());
            Ok(vec![TimelineLine::new(
                "status",
                format!("handled {command}"),
                true,
            )])
        }

        fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
            self.responses.push(payload.clone());
            Ok(())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    for request in [
        json!({"path": "/tui/select-session", "body": {"sessionID": "session_existing"}}),
        json!({"path": "/tui/rename-session", "body": {"title": "New title"}}),
        json!({"path": "/tui/archive-session", "body": {}}),
        json!({"path": "/tui/unarchive-session", "body": {}}),
        json!({"path": "/tui/fork-session", "body": {}}),
        json!({"path": "/tui/session-children", "body": {}}),
        json!({"path": "/tui/share-session", "body": {}}),
        json!({"path": "/tui/unshare-session", "body": {}}),
        json!({"path": "/tui/compact-session", "body": {}}),
        json!({"path": "/tui/session-details", "body": {}}),
        json!({"path": "/tui/undo-session", "body": {}}),
        json!({"path": "/tui/redo-session", "body": {}}),
        json!({"path": "/tui/publish", "body": {"type": "tui.session.delete"}}),
    ] {
        handle_remote_control_request(&mut state, &mut handler, &request);
    }

    assert_eq!(
        handler.commands,
        vec![
            "/resume session_existing",
            "/rename New title",
            "/archive",
            "/unarchive",
            "/fork",
            "/children",
            "/share",
            "/unshare",
            "/compact",
            "/details",
            "/undo",
            "/redo",
            "/delete",
        ]
    );
    assert_eq!(handler.responses.len(), handler.commands.len());
    assert!(
        handler
            .responses
            .iter()
            .all(|payload| payload["ok"] == json!(true))
    );
}
