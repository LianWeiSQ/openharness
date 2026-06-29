#[test]
fn local_interaction_commands_are_forwarded_to_terminal_handler() {
    #[derive(Default)]
    struct CaptureHandler {
        approvals: Vec<Value>,
        questions: Vec<Value>,
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
            self.approvals.push(payload.clone());
            Ok(Vec::new())
        }

        fn handle_question_response(
            &mut self,
            payload: &Value,
        ) -> Result<Vec<TimelineLine>, String> {
            self.questions.push(payload.clone());
            Ok(Vec::new())
        }
    }

    let mut state = TuiState::new();
    let mut handler = CaptureHandler::default();
    state.apply_app_event(&json!({
        "method": "turn/approval_requested",
        "params": {
            "session_id": "session_1",
            "turn_id": "turn_1",
            "approval": {
                "request_id": "approval_4",
                "turn_id": "turn_1",
                "session_id": "session_1",
                "tool_name": "bash"
            }
        }
    }));

    assert!(
        handle_local_state_command("/allow always", &mut state, &mut handler)
            .expect("allow command should be handled")
    );

    assert_eq!(handler.approvals.len(), 1);
    assert_eq!(handler.approvals[0]["request_id"], json!("approval_4"));
    assert_eq!(handler.approvals[0]["turn_id"], json!("turn_1"));
    assert_eq!(handler.approvals[0]["action"], json!("allow"));
    assert_eq!(handler.approvals[0]["scope"], json!("always"));

    state.apply_app_event(&json!({
        "method": "item/question/requested",
        "params": {
            "event": {
                "request_id": "question_4",
                "turn_id": "turn_1",
                "session_id": "session_1",
                "questions": [{"question": "Mode?"}]
            }
        }
    }));

    assert!(
        handle_local_state_command("/answer Safe path", &mut state, &mut handler)
            .expect("answer command should be handled")
    );

    assert_eq!(handler.questions.len(), 1);
    assert_eq!(handler.questions[0]["request_id"], json!("question_4"));
    assert_eq!(handler.questions[0]["turn_id"], json!("turn_1"));
    assert_eq!(handler.questions[0]["answers"], json!([["Safe path"]]));
}
