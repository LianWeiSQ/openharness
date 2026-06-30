#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_core_crate() {
        assert_eq!(crate_name(), "openagent-app-server");
        assert_eq!(core_crate_name(), "openagent-core");
    }

    #[test]
    fn normalizes_approval_and_question_interaction_payloads() {
        assert_eq!(
            approval_response_payload(&json!({"action": "allow_always", "note": "trusted"}))
                .expect("approval payload"),
            json!({"action": "allow", "scope": "always", "note": "trusted"})
        );
        assert_eq!(
            approval_response_payload(&json!({"action": "deny", "scope": "once"}))
                .expect("deny payload"),
            json!({"action": "deny", "scope": "once"})
        );
        assert_eq!(
            parse_turn_question_reply_path("/api/turns/turn_1/questions/question_1/reply")
                .expect("question path"),
            ("turn_1".to_string(), "question_1".to_string())
        );
        assert_eq!(
            question_reply_payload(&json!({"answer": "Fast path"})).expect("question reply"),
            json!({"answers": [["Fast path"]], "dismissed": false})
        );
        assert_eq!(
            question_dismiss_payload(&json!({"note": "not needed"})),
            json!({"answers": [], "dismissed": true, "note": "not needed"})
        );
    }

    #[test]
    fn queues_tui_approval_and_question_controls() {
        assert_eq!(
            tui_control_request_for_path(
                "/tui/respond-approval",
                &json!({"action": "allow", "scope": "always"}),
            )
            .expect("approval control")
            .to_value(),
            json!({"path": "/tui/respond-approval", "body": {"action": "allow", "scope": "always"}})
        );
        assert_eq!(
            publish_to_control(
                &json!({"type": "tui.question.reply", "properties": {"answer": "Safe path"}}),
            )
            .expect("publish question reply"),
            (
                "question.reply".to_string(),
                json!({"answers": [["Safe path"]], "dismissed": false})
            )
        );
    }
}
