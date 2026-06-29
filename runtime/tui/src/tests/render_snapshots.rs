#[test]
fn terminal_render_snapshot_contains_permission_overlay() {
    let backend = TestBackend::new(96, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = TuiState::new();
    state.apply_app_event(&json!({
        "method": "turn/approval_requested",
        "params": {
            "approval": {
                "request_id": "approval_overlay",
                "turn_id": "turn_1",
                "tool_name": "write",
                "tool_input": {"file_path": "src/core.rs"},
                "preview": {
                    "kind": "file",
                    "path": "src/core.rs",
                    "diff": "+changed"
                }
            }
        }
    }));

    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw frame");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("Interaction: Approval"));
    assert!(rendered.contains("Allow once"));
    assert!(rendered.contains("Always allow"));
    assert!(rendered.contains("Deny"));
    assert!(rendered.contains("write"));
}

#[test]
fn terminal_render_snapshot_contains_file_picker_overlay() {
    let backend = TestBackend::new(96, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = TuiState::new();
    state.open_file_picker(
        "main",
        vec![
            ComposerFileCandidate {
                reference: "@src/main.rs".to_string(),
                kind: "file".to_string(),
            },
            ComposerFileCandidate {
                reference: "@images/map.png".to_string(),
                kind: "image".to_string(),
            },
        ],
    );

    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw frame");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("Composer: Files"));
    assert!(rendered.contains("Query"));
    assert!(rendered.contains("@src/main.rs"));
    assert!(rendered.contains("@images/map.png"));
}

#[test]
fn terminal_render_snapshot_contains_session_picker_overlay() {
    let backend = TestBackend::new(96, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = TuiState::new();
    state.open_session_picker(
        "alp",
        vec![
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
                "status": "running",
                "message_count": 4,
                "workspace": "/tmp/alpine"
            }),
        ],
    );

    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw frame");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("Sessions"));
    assert!(rendered.contains("Query"));
    assert!(rendered.contains("session_alpha"));
    assert!(rendered.contains("Alpine"));
}

#[test]
fn terminal_render_snapshot_contains_session_management_views() {
    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = TuiState::new();
    state.open_session_picker(
        "alp",
        vec![json!({
            "session_id": "session_child",
            "title": "Alpha child",
            "status": "idle",
            "message_count": 2,
            "workspace": "/tmp/alpha",
            "model": "server-local",
            "agent": "reviewer",
            "shared": true,
            "metadata": {"parent_session_id": "session_alpha"}
        })],
    );
    state.session_picker.as_mut().expect("session picker").mode = SessionPickerMode::Details;

    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw details frame");
    let details = format!("{:?}", terminal.backend().buffer());
    assert!(details.contains("Details"));
    assert!(details.contains("parent=session_alpha"));
    assert!(details.contains("share=shared"));

    let picker = state.session_picker.as_mut().expect("session picker");
    picker.mode = SessionPickerMode::Actions;
    picker.action_selected = 3;
    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw actions frame");
    let actions = format!("{:?}", terminal.backend().buffer());
    assert!(actions.contains("Actions"));
    assert!(actions.contains("Archive"));
    assert!(actions.contains("confirm"));

    state.session_picker.as_mut().expect("session picker").mode =
        SessionPickerMode::Confirm(SessionPickerAction::Delete);
    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw confirm frame");
    let confirm = format!("{:?}", terminal.backend().buffer());
    assert!(confirm.contains("Confirm"));
    assert!(confirm.contains("Delete session_child"));
}

#[test]
fn terminal_render_snapshot_contains_model_picker_overlay() {
    let backend = TestBackend::new(96, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = TuiState::new();
    state.open_model_picker(
        "deep",
        vec![
            json!({
                "id": "server-local",
                "provider_id": "openagent",
                "name": "Server Local",
                "default": true
            }),
            json!({
                "id": "deep-model",
                "provider_id": "openagent",
                "name": "Deep Model"
            }),
        ],
    );

    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw frame");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("Models"));
    assert!(rendered.contains("Query"));
    assert!(rendered.contains("deep-model"));
    assert!(rendered.contains("Deep Model"));
}

#[test]
fn terminal_render_snapshot_contains_agent_picker_overlay() {
    let backend = TestBackend::new(96, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = TuiState::new();
    state.open_agent_picker(
        "rev",
        vec![
            json!({
                "id": "server",
                "name": "Server",
                "description": "Default server agent",
                "default": true
            }),
            json!({
                "id": "reviewer",
                "name": "Reviewer",
                "description": "Review code"
            }),
        ],
    );

    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw frame");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("Agents"));
    assert!(rendered.contains("Query"));
    assert!(rendered.contains("reviewer"));
    assert!(rendered.contains("Reviewer"));
}

#[test]
fn terminal_render_snapshot_contains_choice_picker_overlay() {
    let backend = TestBackend::new(96, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = TuiState::new();
    state.open_choice_picker(
        ChoicePickerKind::Variant,
        "dee",
        vec!["default".to_string(), "deep".to_string()],
    );

    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw frame");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("Variants"));
    assert!(rendered.contains("Query"));
    assert!(rendered.contains("deep"));
    assert!(rendered.contains("Enter select"));
}

#[test]
fn terminal_render_snapshot_contains_theme_picker_overlay() {
    let backend = TestBackend::new(96, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = TuiState::new();
    state.set_theme("midnight");
    state.open_choice_picker(ChoicePickerKind::Theme, "", default_theme_names());

    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw frame");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("Themes"));
    assert!(rendered.contains("Query"));
    assert!(rendered.contains("midnight  current"));
    assert!(rendered.contains("high-contrast"));
}

#[test]
fn terminal_render_snapshot_contains_color_scheme_picker_overlay() {
    let backend = TestBackend::new(96, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = TuiState::new();
    state.set_color_scheme("dark");
    state.open_choice_picker(
        ChoicePickerKind::ThemeScheme,
        "",
        default_color_scheme_names(),
    );

    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw frame");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("Color Schemes"));
    assert!(rendered.contains("Query"));
    assert!(rendered.contains("dark  current"));
    assert!(rendered.contains("system"));
}
