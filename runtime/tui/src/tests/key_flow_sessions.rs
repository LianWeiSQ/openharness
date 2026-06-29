#[test]
fn key_event_flow_submits_prompt_and_uses_history() {
    #[derive(Default)]
    struct CaptureHandler {
        prompts: Vec<String>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, prompt: &str) -> Result<Vec<TimelineLine>, String> {
            self.prompts.push(prompt.to_string());
            Ok(vec![TimelineLine::new("status", "submitted", true)])
        }

        fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    for ch in "hello".chars() {
        handle_key_event(
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("char event");
    }
    handle_key_event(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("enter event");
    assert_eq!(handler.prompts, vec!["hello".to_string()]);

    handle_key_event(
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("history up");
    assert_eq!(state.input_buffer, "hello");
    handle_key_event(
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("history down");
    assert_eq!(state.input_buffer, "");
}

#[test]
fn key_event_flow_opens_file_picker_filters_and_attaches() {
    #[derive(Default)]
    struct CaptureHandler {
        searches: Vec<String>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn search_files(&mut self, query: &str) -> Result<Vec<ComposerFileCandidate>, String> {
            self.searches.push(query.to_string());
            let candidates = match query {
                "ma" => vec![
                    ComposerFileCandidate {
                        reference: "@src/main.rs".to_string(),
                        kind: "file".to_string(),
                    },
                    ComposerFileCandidate {
                        reference: "@images/map.png".to_string(),
                        kind: "image".to_string(),
                    },
                ],
                "mam" => vec![ComposerFileCandidate {
                    reference: "@src/main.rs".to_string(),
                    kind: "file".to_string(),
                }],
                _ => vec![
                    ComposerFileCandidate {
                        reference: "@src/core.rs".to_string(),
                        kind: "file".to_string(),
                    },
                    ComposerFileCandidate {
                        reference: "@docs/guide.md".to_string(),
                        kind: "file".to_string(),
                    },
                ],
            };
            Ok(candidates)
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    for ch in "/files ma".chars() {
        handle_key_event(
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("char event");
    }
    handle_key_event(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("open picker");
    assert_eq!(handler.searches, vec!["ma".to_string()]);
    assert_eq!(
        state.file_picker.as_ref().expect("picker").query.as_str(),
        "ma"
    );

    handle_key_event(
        KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("filter picker");
    assert_eq!(
        state.file_picker.as_ref().expect("picker").query.as_str(),
        "mam"
    );
    assert_eq!(handler.searches, vec!["ma".to_string(), "mam".to_string()]);

    handle_key_event(
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("backspace filter");
    handle_key_event(
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("select image");
    handle_key_event(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("attach selected file");

    assert!(state.file_picker.is_none());
    assert_eq!(state.input_buffer, "@images/map.png ");
}

#[test]
fn key_event_flow_opens_session_picker_filters_and_resumes() {
    #[derive(Default)]
    struct CaptureHandler {
        searches: Vec<String>,
        commands: Vec<String>,
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

        fn search_sessions(&mut self, query: &str) -> Result<Vec<Value>, String> {
            self.searches.push(query.to_string());
            let sessions = match query {
                "al" => vec![
                    json!({
                        "session_id": "session_alpha",
                        "title": "Alpha",
                        "status": "idle",
                        "message_count": 2,
                        "workspace": "/tmp/alpha"
                    }),
                    json!({
                        "session_id": "session_alpine",
                        "title": "Alpine",
                        "status": "idle",
                        "message_count": 3,
                        "workspace": "/tmp/alpine"
                    }),
                ],
                "alp" => vec![json!({
                    "session_id": "session_alpha",
                    "title": "Alpha",
                    "status": "idle",
                    "message_count": 2,
                    "workspace": "/tmp/alpha"
                })],
                _ => Vec::new(),
            };
            Ok(sessions)
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    send_key_text("/sessions al", &mut state, &mut handler).expect("type sessions command");
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("open session picker");

    assert_eq!(handler.searches, vec!["al".to_string()]);
    assert_eq!(
        state
            .session_picker
            .as_ref()
            .expect("session picker")
            .candidates
            .len(),
        2
    );

    press_key(KeyCode::Char('p'), &mut state, &mut handler).expect("filter picker");
    assert_eq!(handler.searches, vec!["al".to_string(), "alp".to_string()]);
    assert_eq!(
        state.session_picker.as_ref().expect("session picker").query,
        "alp"
    );

    press_key(KeyCode::Enter, &mut state, &mut handler).expect("resume selected session");

    assert!(state.session_picker.is_none());
    assert_eq!(state.session_id.as_deref(), Some("session_alpha"));
    assert_eq!(handler.commands, vec!["/resume session_alpha".to_string()]);
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("handled /resume session_alpha"))
    );
}

#[test]
fn key_event_flow_session_picker_manages_sessions_interactively() {
    struct CaptureHandler {
        current: Option<String>,
        searches: Vec<String>,
        commands: Vec<String>,
        sessions: Vec<Value>,
    }

    impl Default for CaptureHandler {
        fn default() -> Self {
            Self {
                current: None,
                searches: Vec::new(),
                commands: Vec::new(),
                sessions: vec![
                    json!({
                        "session_id": "session_alpha",
                        "title": "Alpha",
                        "status": "idle",
                        "message_count": 2,
                        "workspace": "/tmp/alpha",
                        "child_count": 1
                    }),
                    json!({
                        "session_id": "session_child",
                        "title": "Alpha child",
                        "status": "idle",
                        "message_count": 1,
                        "workspace": "/tmp/alpha",
                        "metadata": {"parent_session_id": "session_alpha"}
                    }),
                ],
            }
        }
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
            self.commands.push(command.to_string());
            if let Some(session_id) = command.strip_prefix("/resume ") {
                self.current = Some(session_id.to_string());
            } else if let Some(title) = command.strip_prefix("/rename ") {
                if let Some(current) = self.current.as_deref()
                    && let Some(session) = self.sessions.iter_mut().find(|session| {
                        session_id_from_payload(session).as_deref() == Some(current)
                    })
                {
                    session["title"] = json!(title);
                }
            } else if command == "/archive" {
                if let Some(current) = self.current.as_deref()
                    && let Some(session) = self.sessions.iter_mut().find(|session| {
                        session_id_from_payload(session).as_deref() == Some(current)
                    })
                {
                    session["archived"] = json!(true);
                }
            } else if command == "/delete"
                && let Some(current) = self.current.as_deref()
            {
                self.sessions
                    .retain(|session| session_id_from_payload(session).as_deref() != Some(current));
                self.current = None;
            } else if command == "/parent" {
                self.current = Some("session_alpha".to_string());
            }
            Ok(vec![TimelineLine::new(
                "status",
                format!("handled {command}"),
                true,
            )])
        }

        fn search_sessions(&mut self, query: &str) -> Result<Vec<Value>, String> {
            self.searches.push(query.to_string());
            Ok(self.sessions.clone())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    open_session_picker_from_handler(&mut state, &mut handler, "").expect("open sessions");

    handle_key_event(
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        &mut state,
        &mut handler,
    )
    .expect("show details");
    assert_eq!(
        state.session_picker.as_ref().expect("session picker").mode,
        SessionPickerMode::Details
    );

    press_key(KeyCode::Right, &mut state, &mut handler).expect("open actions");
    press_key(KeyCode::Down, &mut state, &mut handler).expect("details action");
    press_key(KeyCode::Down, &mut state, &mut handler).expect("rename action");
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("start rename");
    for _ in 0..5 {
        press_key(KeyCode::Backspace, &mut state, &mut handler).expect("clear title");
    }
    send_key_text("Alpha Prime", &mut state, &mut handler).expect("type new title");
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("rename session");

    assert_eq!(
        handler.commands,
        vec![
            "/resume session_alpha".to_string(),
            "/rename Alpha Prime".to_string(),
        ]
    );
    assert_eq!(
        state
            .session_picker
            .as_ref()
            .expect("session picker")
            .candidates[0]["title"],
        json!("Alpha Prime")
    );

    press_key(KeyCode::Right, &mut state, &mut handler).expect("open actions again");
    for _ in 0..3 {
        press_key(KeyCode::Down, &mut state, &mut handler).expect("archive action");
    }
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("confirm archive");
    assert!(matches!(
        state.session_picker.as_ref().expect("session picker").mode,
        SessionPickerMode::Confirm(SessionPickerAction::Archive)
    ));
    press_key(KeyCode::Char('y'), &mut state, &mut handler).expect("archive session");
    assert!(handler.commands.contains(&"/archive".to_string()));
    assert_eq!(
        state
            .session_picker
            .as_ref()
            .expect("session picker")
            .candidates[0]["archived"],
        json!(true)
    );

    state
        .session_picker
        .as_mut()
        .expect("session picker")
        .selected = 1;
    press_key(KeyCode::Right, &mut state, &mut handler).expect("open child actions");
    for _ in 0..8 {
        press_key(KeyCode::Down, &mut state, &mut handler).expect("parent action");
    }
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("navigate parent");
    assert_eq!(state.session_id.as_deref(), Some("session_alpha"));
    assert!(
        handler
            .commands
            .windows(2)
            .any(|window| { window[0] == "/resume session_child" && window[1] == "/parent" })
    );

    press_key(KeyCode::Delete, &mut state, &mut handler).expect("delete confirm");
    assert!(matches!(
        state.session_picker.as_ref().expect("session picker").mode,
        SessionPickerMode::Confirm(SessionPickerAction::Delete)
    ));
}
