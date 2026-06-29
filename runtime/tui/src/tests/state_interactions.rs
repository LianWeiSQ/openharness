#[test]
fn exposes_command_boundary() {
    assert_eq!(crate_name(), "openagent-tui");
    assert_eq!(command_name(), "openagent-tui");
    assert_eq!(client_crate_name(), "openagent-app-server-client");
    assert_eq!(server_crate_name(), "openagent-app-server");
}

#[test]
fn approval_events_render_diff_preview_and_support_allow_always() {
    let mut state = TuiState::new();
    let applied = state.apply_app_event(&json!({
        "method": "turn/approval_requested",
        "params": {
            "approval": {
                "request_id": "approval_1",
                "turn_id": "turn_1",
                "session_id": "session_1",
                "tool_name": "write",
                "tool_input": {"file_path": "src/core.rs"},
                "preview": {
                    "kind": "file",
                    "path": "src/core.rs",
                    "status": "modified",
                    "diff": "--- a/src/core.rs\n+++ b/src/core.rs\n+hello"
                }
            }
        }
    }));

    assert_eq!(applied["applied"], json!(true));
    assert_eq!(state.status, "approval pending");
    let timeline = state
        .timeline
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(timeline.contains("approval required: write"));
    assert!(timeline.contains("diff:"));
    assert!(timeline.contains("+hello"));

    let response = state.respond_active_approval("allow", Some("always"), None);

    assert_eq!(response["applied"], json!(true));
    assert_eq!(response["payload"]["action"], json!("allow"));
    assert_eq!(response["payload"]["scope"], json!("always"));
    assert_eq!(response["payload"]["request_id"], json!("approval_1"));
    assert!(state.active_approval.is_none());
}

#[test]
fn patch_events_render_structured_diff_and_undo_redo_markers() {
    let mut state = TuiState::new();
    let applied = state.apply_app_event(&json!({
        "method": "patch/detected",
        "params": {
            "session_id": "session_1",
            "turn_id": "turn_1",
            "patch": {
                "id": "patch_1",
                "path": "src/core.rs",
                "status": "modified",
                "diff": "--- a/src/core.rs\n+++ b/src/core.rs\n@@ -1,1 +1,1 @@\n-old\n+new"
            }
        }
    }));

    assert_eq!(applied["applied"], json!(true));
    assert_eq!(state.status, "patch detected");
    let kinds = state
        .timeline
        .iter()
        .map(|line| line.kind.as_str())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"patch"));
    assert!(kinds.contains(&"diff-meta"));
    assert!(kinds.contains(&"diff-hunk"));
    assert!(kinds.contains(&"diff-del"));
    assert!(kinds.contains(&"diff-add"));
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("actions: /undo /redo"))
    );

    let detail_lines = diff_detail_lines(&json!({
        "undo_count": 1,
        "redo_count": 1,
        "latest": {
            "id": "patch_1",
            "path": "src/core.rs",
            "status": "modified",
            "diff": "+new"
        }
    }));
    assert_eq!(detail_lines[0].kind, "patch");
    assert!(detail_lines[0].text.contains("/undo"));
    assert!(detail_lines[0].text.contains("/redo"));
    assert!(detail_lines.iter().any(|line| line.kind == "diff-add"));
}

#[test]
fn approval_can_be_denied_with_note_from_command() {
    let mut state = TuiState::new();
    state.apply_app_event(&json!({
        "method": "turn/approval_requested",
        "params": {
            "approval": {
                "request_id": "approval_2",
                "turn_id": "turn_1",
                "tool_name": "bash",
                "tool_input": {"command": "rm -rf target"}
            }
        }
    }));
    state.input_buffer = "/deny risky command".to_string();

    assert!(!state.submit());

    let response = state.approval_responses.last().expect("approval response");
    assert_eq!(response["action"], json!("deny"));
    assert_eq!(response["note"], json!("risky command"));
    assert_eq!(state.status, "approval resolved");
}

#[test]
fn question_events_support_answer_and_dismiss() {
    let mut state = TuiState::new();
    let question_event = json!({
        "method": "item/question/requested",
        "params": {
            "event": {
                "type": "question-request",
                "request_id": "question_1",
                "turn_id": "turn_1",
                "session_id": "session_1",
                "tool_call_id": "call_question",
                "questions": [{
                    "header": "Plan",
                    "question": "Which option?",
                    "options": [
                        {"label": "Fast path", "description": "Move quickly"},
                        {"label": "Safe path", "description": "Be conservative"}
                    ]
                }]
            }
        }
    });

    assert_eq!(
        state.apply_app_event(&question_event)["applied"],
        json!(true)
    );
    assert_eq!(state.status, "question pending");
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("Fast path"))
    );

    state.input_buffer = "/answer Safe path".to_string();
    assert!(!state.submit());

    let response = state.question_responses.last().expect("question response");
    assert_eq!(response["answers"], json!([["Safe path"]]));
    assert_eq!(response["request_id"], json!("question_1"));
    assert!(state.active_question.is_none());

    state.apply_app_event(&question_event);
    let dismissed = state.dismiss_active_question(Some("not needed"));
    assert_eq!(dismissed["payload"]["dismissed"], json!(true));
    assert_eq!(dismissed["payload"]["note"], json!("not needed"));
}

#[test]
fn control_requests_can_respond_to_active_interactions() {
    let mut state = TuiState::new();
    state.apply_app_event(&json!({
        "method": "turn/approval_requested",
        "params": {"approval": {"request_id": "approval_3", "tool_name": "write"}}
    }));

    let approval = state.apply_control_request(&json!({
        "path": "/tui/respond-approval",
        "body": {"action": "allow_always"}
    }));

    assert_eq!(approval["applied"], json!(true));
    assert_eq!(approval["payload"]["scope"], json!("always"));

    state.apply_app_event(&json!({
        "method": "item/question/requested",
        "params": {"event": {"request_id": "question_2", "questions": [{"question": "Mode?"}]}}
    }));
    let answer = state.apply_control_request(&json!({
        "path": "/tui/reply-question",
        "body": {"answer": "Fast"}
    }));

    assert_eq!(answer["applied"], json!(true));
    assert_eq!(answer["payload"]["answers"], json!([["Fast"]]));
}

#[test]
fn control_requests_open_model_theme_and_palette_surfaces() {
    let mut state = TuiState::new();
    let models = state.apply_control_request(&json!({
        "path": "/tui/open-models",
        "body": {"models": [{"id": "gpt-test", "name": "GPT Test"}]}
    }));
    assert_eq!(models["applied"], json!(true));
    assert_eq!(state.status, "model picker");
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("gpt-test"))
    );

    let theme = state.apply_control_request(&json!({
        "path": "/tui/select-theme",
        "body": {"theme": "midnight"}
    }));
    assert_eq!(theme["applied"], json!(true));
    assert_eq!(state.config.theme, "midnight");

    let themes = state.apply_control_request(&json!({
        "path": "/tui/open-themes",
        "body": {"themes": ["default", "midnight", "high-contrast"]}
    }));
    assert_eq!(themes["applied"], json!(true));
    assert_eq!(state.status, "theme picker");
    let picker = state.choice_picker.as_ref().expect("theme picker");
    assert_eq!(picker.kind, ChoicePickerKind::Theme);
    assert_eq!(
        picker.candidates,
        vec![
            "default".to_string(),
            "midnight".to_string(),
            "high-contrast".to_string()
        ]
    );

    let schemes = state.apply_control_request(&json!({
        "path": "/tui/open-theme-schemes",
        "body": {}
    }));
    assert_eq!(schemes["applied"], json!(true));
    assert_eq!(state.status, "theme-scheme picker");
    let picker = state.choice_picker.as_ref().expect("scheme picker");
    assert_eq!(picker.kind, ChoicePickerKind::ThemeScheme);
    assert_eq!(picker.candidates, default_color_scheme_names());

    let selected_scheme = state.apply_control_request(&json!({
        "path": "/tui/select-theme-scheme",
        "body": {"scheme": "dark"}
    }));
    assert_eq!(selected_scheme["applied"], json!(true));
    assert_eq!(state.config.color_scheme, "dark");

    let cycled_scheme = state.apply_control_request(&json!({
        "path": "/tui/cycle-theme-scheme",
        "body": {}
    }));
    assert_eq!(cycled_scheme["applied"], json!(true));
    assert_eq!(state.config.color_scheme, "system");

    let published_scheme = state.apply_control_request(&json!({
        "path": "/tui/publish",
        "body": {"type": "theme.scheme.light"}
    }));
    assert_eq!(published_scheme["applied"], json!(true));
    assert_eq!(state.config.color_scheme, "light");

    let palette = state.apply_control_request(&json!({
        "path": "/tui/open-palette",
        "body": {"query": "model"}
    }));
    assert_eq!(palette["applied"], json!(true));
    assert_eq!(state.status, "palette open");
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("/models"))
    );

    let agents = state.apply_control_request(&json!({
        "path": "/tui/open-agents",
        "body": {"agents": [{"id": "reviewer", "name": "Reviewer", "description": "Review code"}]}
    }));
    assert_eq!(agents["applied"], json!(true));
    assert_eq!(state.status, "agent picker");
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("reviewer"))
    );

    let variants = state.apply_control_request(&json!({
        "path": "/tui/open-variants",
        "body": {"variants": ["default", "deep"]}
    }));
    assert_eq!(variants["applied"], json!(true));
    assert_eq!(state.status, "variant picker");
    assert!(
        variants["variants"]
            .as_array()
            .is_some_and(|items| items.len() == 2)
    );

    let thinking = state.apply_control_request(&json!({
        "path": "/tui/open-thinking",
        "body": {"levels": ["low", "high"]}
    }));
    assert_eq!(thinking["applied"], json!(true));
    assert_eq!(state.status, "thinking picker");
    assert!(
        thinking["levels"]
            .as_array()
            .is_some_and(|items| items.len() == 2)
    );
}
