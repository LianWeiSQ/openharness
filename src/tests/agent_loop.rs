use std::{fs, path::PathBuf};

use openagent_core::{ScriptedLoopInput, run_scripted_agent_loop};
use serde_json::{Value, json};

#[test]
fn agent_loop_fixture_matches_python_oracle() {
    let fixture = read_fixture();
    let scenarios = fixture["scenarios"]
        .as_object()
        .expect("agent loop scenarios object");
    for name in [
        "multi_step_tool",
        "runtime_warning",
        "question_pause_reply",
        "model_retry",
        "doom_loop",
    ] {
        let scenario = scenarios.get(name).expect("scenario exists");
        let input: ScriptedLoopInput =
            serde_json::from_value(scenario["input"].clone()).expect("scenario input parses");
        let output = run_scripted_agent_loop(&input);

        assert_eq!(json!(output.events), scenario["events"], "{name} events");
        assert_eq!(
            json!(output.event_types),
            scenario["event_types"],
            "{name} event types"
        );
        assert_eq!(
            json!(output.model_call_count),
            scenario["model_call_count"],
            "{name} model calls"
        );
        assert_eq!(
            json!(output.seen_tools_by_call),
            scenario["seen_tools_by_call"],
            "{name} seen tools"
        );
        assert_eq!(
            json!(output.seen_max_output_tokens_by_call),
            scenario["seen_max_output_tokens_by_call"],
            "{name} max output tokens"
        );
        assert_eq!(
            json!(output.pause_statuses),
            scenario["pause_statuses"],
            "{name} pause statuses"
        );
        assert_eq!(
            json!(output.final_session_status),
            scenario["final_session_status"],
            "{name} final status"
        );
    }
}

#[test]
fn agent_loop_reports_model_error_after_retry_budget() {
    let input = ScriptedLoopInput {
        user_text: "fail twice".to_string(),
        script: vec![
            openagent_core::ScriptedLoopCall {
                events: Vec::new(),
                error: Some("first failure".to_string()),
            },
            openagent_core::ScriptedLoopCall {
                events: Vec::new(),
                error: Some("second failure".to_string()),
            },
        ],
        tools: Vec::new(),
        options: Default::default(),
        max_steps: 3,
        doom_loop_threshold: 3,
        reply_questions: false,
    };

    let output = run_scripted_agent_loop(&input);
    assert_eq!(output.model_call_count, 2);
    assert_eq!(
        output.events.last(),
        Some(&json!({"type": "error", "error": "second failure"}))
    );
    assert_eq!(output.final_session_status, "stop");
}

fn read_fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/rust_rewrite/agent_loop.json");
    let raw = fs::read_to_string(path).expect("read agent loop fixture");
    serde_json::from_str(&raw).expect("parse agent loop fixture")
}
