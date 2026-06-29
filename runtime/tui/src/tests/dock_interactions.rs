#[test]
fn key_event_flow_answers_question_option_from_dock() {
    #[derive(Default)]
    struct CaptureHandler {
        question_payloads: Vec<Value>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_question_response(
            &mut self,
            payload: &Value,
        ) -> Result<Vec<TimelineLine>, String> {
            self.question_payloads.push(payload.clone());
            Ok(vec![TimelineLine::new("status", "question sent", true)])
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    state.apply_app_event(&json!({
        "method": "item/question/requested",
        "params": {
            "event": {
                "request_id": "question_dock",
                "turn_id": "turn_1",
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
    }));

    handle_key_event(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("question dock key");

    assert_eq!(handler.question_payloads.len(), 1);
    assert_eq!(
        handler.question_payloads[0]["answers"],
        json!([["Safe path"]])
    );
    assert!(state.active_question.is_none());
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("question sent"))
    );
}

#[test]
fn key_event_flow_approves_from_dock() {
    #[derive(Default)]
    struct CaptureHandler {
        approval_payloads: Vec<Value>,
    }

    impl TerminalEventHandler for CaptureHandler {
        fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
            Ok(Vec::new())
        }

        fn handle_approval_response(
            &mut self,
            payload: &Value,
        ) -> Result<Vec<TimelineLine>, String> {
            self.approval_payloads.push(payload.clone());
            Ok(vec![TimelineLine::new("status", "approval sent", true)])
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    state.apply_app_event(&json!({
        "method": "turn/approval_requested",
        "params": {
            "approval": {
                "request_id": "approval_dock",
                "turn_id": "turn_1",
                "tool_name": "bash",
                "tool_input": {"command": "printf ok"}
            }
        }
    }));

    handle_key_event(
        KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
        &mut state,
        &mut handler,
    )
    .expect("approval dock key");

    assert_eq!(handler.approval_payloads.len(), 1);
    assert_eq!(handler.approval_payloads[0]["action"], json!("allow"));
    assert_eq!(handler.approval_payloads[0]["scope"], json!("once"));
    assert!(state.active_approval.is_none());
}
