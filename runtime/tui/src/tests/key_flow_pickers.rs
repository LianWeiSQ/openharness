#[test]
fn key_event_flow_opens_model_picker_filters_and_selects() {
    #[derive(Default)]
    struct CaptureHandler {
        model_fetches: usize,
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

        fn list_models(&mut self) -> Result<Value, String> {
            self.model_fetches += 1;
            Ok(json!({
                "models": [
                    {"id": "server-local", "provider_id": "openagent", "name": "Server Local"},
                    {"id": "deep-model", "provider_id": "openagent", "name": "Deep Model"}
                ]
            }))
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    send_key_text("/models", &mut state, &mut handler).expect("type models command");
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("open model picker");

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

    press_key(KeyCode::Char('d'), &mut state, &mut handler).expect("filter picker");
    assert_eq!(
        state
            .model_picker
            .as_ref()
            .expect("model picker")
            .candidates
            .len(),
        1
    );
    assert_eq!(
        state
            .model_picker
            .as_ref()
            .expect("model picker")
            .candidates[0]["id"],
        json!("deep-model")
    );

    press_key(KeyCode::Enter, &mut state, &mut handler).expect("select model");

    assert!(state.model_picker.is_none());
    assert_eq!(handler.commands, vec!["/models deep-model".to_string()]);
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("handled /models deep-model"))
    );
}

#[test]
fn key_event_flow_opens_agent_picker_filters_and_selects() {
    #[derive(Default)]
    struct CaptureHandler {
        agent_fetches: usize,
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

        fn list_agents(&mut self) -> Result<Value, String> {
            self.agent_fetches += 1;
            Ok(json!({
                "agents": [
                    {"id": "server", "name": "Server", "description": "Default server agent"},
                    {"id": "reviewer", "name": "Reviewer", "description": "Review code"}
                ]
            }))
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    send_key_text("/agents", &mut state, &mut handler).expect("type agents command");
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("open agent picker");

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

    send_key_text("rev", &mut state, &mut handler).expect("filter picker");
    assert_eq!(
        state
            .agent_picker
            .as_ref()
            .expect("agent picker")
            .candidates[0]["id"],
        json!("reviewer")
    );

    press_key(KeyCode::Enter, &mut state, &mut handler).expect("select agent");

    assert!(state.agent_picker.is_none());
    assert_eq!(handler.commands, vec!["/agent reviewer".to_string()]);
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("handled /agent reviewer"))
    );
}

#[test]
fn key_event_flow_opens_variant_and_thinking_pickers() {
    #[derive(Default)]
    struct CaptureHandler {
        model_fetches: usize,
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

        fn list_models(&mut self) -> Result<Value, String> {
            self.model_fetches += 1;
            Ok(json!({
                "models": [],
                "variants": ["default", "deep"],
                "thinking": ["low", "high"]
            }))
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();

    send_key_text("/variant", &mut state, &mut handler).expect("type variant command");
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("open variant picker");
    assert_eq!(handler.model_fetches, 1);
    assert_eq!(
        state.choice_picker.as_ref().expect("variant picker").kind,
        ChoicePickerKind::Variant
    );

    send_key_text("dee", &mut state, &mut handler).expect("filter variant picker");
    assert_eq!(
        state
            .choice_picker
            .as_ref()
            .expect("variant picker")
            .candidates,
        vec!["deep".to_string()]
    );
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("select variant");

    send_key_text("/thinking", &mut state, &mut handler).expect("type thinking command");
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("open thinking picker");
    assert_eq!(handler.model_fetches, 2);
    assert_eq!(
        state.choice_picker.as_ref().expect("thinking picker").kind,
        ChoicePickerKind::Thinking
    );

    send_key_text("hi", &mut state, &mut handler).expect("filter thinking picker");
    assert_eq!(
        state
            .choice_picker
            .as_ref()
            .expect("thinking picker")
            .candidates,
        vec!["high".to_string()]
    );
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("select thinking");

    assert!(state.choice_picker.is_none());
    assert_eq!(
        handler.commands,
        vec!["/variant deep".to_string(), "/thinking high".to_string()]
    );
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("handled /thinking high"))
    );
}

#[test]
fn key_event_flow_opens_theme_picker_filters_and_selects() {
    #[derive(Default)]
    struct CaptureHandler {
        commands: Vec<String>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
            self.commands.push(command.to_string());
            Ok(Vec::new())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();

    send_key_text("/themes", &mut state, &mut handler).expect("type themes command");
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("open theme picker");

    let picker = state.choice_picker.as_ref().expect("theme picker");
    assert_eq!(picker.kind, ChoicePickerKind::Theme);
    assert_eq!(picker.candidates, default_theme_names());

    send_key_text("high", &mut state, &mut handler).expect("filter theme picker");
    assert_eq!(
        state
            .choice_picker
            .as_ref()
            .expect("theme picker")
            .candidates,
        vec!["high-contrast".to_string()]
    );
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("select theme");

    assert!(state.choice_picker.is_none());
    assert_eq!(state.config.theme, "high-contrast");
    assert!(handler.commands.is_empty());
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("theme set to high-contrast"))
    );
}

#[test]
fn key_event_flow_opens_color_scheme_picker_filters_and_selects() {
    #[derive(Default)]
    struct CaptureHandler {
        commands: Vec<String>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
            self.commands.push(command.to_string());
            Ok(Vec::new())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();

    send_key_text("/theme-scheme", &mut state, &mut handler).expect("type color scheme command");
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("open color scheme picker");

    let picker = state.choice_picker.as_ref().expect("color scheme picker");
    assert_eq!(picker.kind, ChoicePickerKind::ThemeScheme);
    assert_eq!(picker.candidates, default_color_scheme_names());

    send_key_text("da", &mut state, &mut handler).expect("filter color scheme picker");
    assert_eq!(
        state
            .choice_picker
            .as_ref()
            .expect("color scheme picker")
            .candidates,
        vec!["dark".to_string()]
    );
    press_key(KeyCode::Enter, &mut state, &mut handler).expect("select color scheme");

    assert!(state.choice_picker.is_none());
    assert_eq!(state.config.color_scheme, "dark");
    assert!(handler.commands.is_empty());
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("color scheme set to dark"))
    );
}

#[test]
fn key_event_flow_at_opens_file_picker_without_touching_commands() {
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
            Ok(vec![
                ComposerFileCandidate {
                    reference: "@src/main.rs".to_string(),
                    kind: "file".to_string(),
                },
                ComposerFileCandidate {
                    reference: "@docs/guide.md".to_string(),
                    kind: "file".to_string(),
                },
            ])
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    state.input_buffer = "review".to_string();
    handle_key_event(
        KeyEvent::new(KeyCode::Char('@'), KeyModifiers::SHIFT),
        &mut state,
        &mut handler,
    )
    .expect("@ opens file picker");

    assert_eq!(handler.searches, vec!["".to_string()]);
    assert_eq!(state.input_buffer, "review");
    assert_eq!(
        state
            .file_picker
            .as_ref()
            .expect("file picker")
            .candidates
            .len(),
        2
    );

    handle_key_event(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("attach first candidate");

    assert!(state.file_picker.is_none());
    assert_eq!(state.input_buffer, "review @src/main.rs ");

    state.input_buffer = "/rename ".to_string();
    handle_key_event(
        KeyEvent::new(KeyCode::Char('@'), KeyModifiers::SHIFT),
        &mut state,
        &mut handler,
    )
    .expect("@ stays literal in commands");
    assert_eq!(state.input_buffer, "/rename @");
    assert_eq!(handler.searches, vec!["".to_string()]);
}
