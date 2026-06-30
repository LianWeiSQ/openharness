use super::common::*;
use super::fixtures::*;

#[test]
fn session_trace_observability_fixture_matches_legacy_oracle() {
    let fixture = fixture();

    assert_eq!(fixture["session"]["todo"], value(fixture_todo()));
    assert_eq!(fixture["session"]["message"], value(fixture_message()));
    assert_eq!(fixture["session"]["event"], fixture_session_event());
    assert_eq!(fixture["session"]["part"], fixture_session_part());
    assert_eq!(fixture["session"]["state"], fixture_session_state());
    assert_eq!(fixture["session"]["summary"], fixture_session_summary());

    assert_eq!(fixture["trace"]["config"], value(fixture_trace_config()));
    assert_eq!(fixture["trace"]["run"], value(fixture_run_record()));
    assert_eq!(fixture["trace"]["event"], value(fixture_trace_event()));
    assert_eq!(
        fixture["trace"]["rendered_summary"],
        json!(render_trace_summary(&fixture["trace"]["summary"]))
    );

    assert_eq!(
        fixture["observability"]["config"],
        value(fixture_observation_config())
    );
    assert_eq!(
        fixture["observability"]["trace"],
        value(fixture_observation_trace())
    );
    assert_eq!(
        fixture["observability"]["event"],
        value(fixture_observation_event())
    );
    assert_eq!(
        fixture["observability"]["input_preview"],
        json!(input_preview(
            json!({"api_key": "secret", "path": "README.md"}),
            80
        ))
    );
    assert_eq!(
        fixture["observability"]["output_stats"],
        value(output_stats("one\ntwo\n"))
    );

    assert_eq!(
        fixture["runtime_logging"]["config"],
        value(fixture_logging_config())
    );
    assert_eq!(
        fixture["runtime_logging"]["record"],
        value(fixture_log_record())
    );

    let warning = fixture_warning_record();
    assert_eq!(
        fixture["runtime_warnings"]["config"],
        value(fixture_warning_config())
    );
    assert_eq!(fixture["runtime_warnings"]["record"], value(&warning));
    assert_eq!(fixture["runtime_warnings"]["event"], warning.to_event());
    assert_eq!(
        fixture["runtime_warnings"]["formatted"],
        json!(format_runtime_warning_event(&warning.to_event()).expect("warning formats"))
    );
}

#[test]
fn trace_recorder_writes_jsonl_summary_and_checkable_run() {
    let root = unique_temp_dir("openagent-trace");
    let run = RunRecord {
        run_id: "run_trace".to_string(),
        trace_id: "trace_trace".to_string(),
        session_id: "session_trace".to_string(),
        agent_name: "agent".to_string(),
        model_id: Some("model".to_string()),
        provider_id: Some("provider".to_string()),
        workspace: Some(root.to_string_lossy().to_string()),
        started_at_ms: 1_781_840_000_000,
    };
    let mut metadata = BTreeMap::new();
    {
        let mut recorder = AgentTraceRecorder::new(
            run,
            Some(TraceConfig {
                root_dir: "runs".to_string(),
                ..TraceConfig::default()
            }),
            &root,
            Some(&mut metadata),
        )
        .expect("trace recorder starts");
        recorder
            .record_event(
                "run.started",
                TraceEventOptions {
                    kind: Some("run".to_string()),
                    ..TraceEventOptions::default()
                },
            )
            .expect("run start records");
        recorder
            .record_event(
                "step.started",
                TraceEventOptions {
                    kind: Some("step".to_string()),
                    ..TraceEventOptions::default()
                },
            )
            .expect("step start records");
        recorder
            .record_event(
                "model.call.started",
                TraceEventOptions {
                    kind: Some("model".to_string()),
                    span_id: Some("model_1".to_string()),
                    ..TraceEventOptions::default()
                },
            )
            .expect("model start records");
        recorder
            .record_event(
                "model.call.finished",
                TraceEventOptions {
                    kind: Some("model".to_string()),
                    span_id: Some("model_1".to_string()),
                    duration_ms: Some(33),
                    attributes: BTreeMap::from([
                        ("api_key".to_string(), json!("secret")),
                        ("input_tokens".to_string(), json!(3)),
                        ("output_tokens".to_string(), json!(4)),
                        ("cost".to_string(), json!(0.01)),
                    ]),
                    ..TraceEventOptions::default()
                },
            )
            .expect("model finish records");
        recorder
            .record_event(
                "step.finished",
                TraceEventOptions {
                    kind: Some("step".to_string()),
                    ..TraceEventOptions::default()
                },
            )
            .expect("step finish records");
        recorder
            .finish_run("completed", BTreeMap::new())
            .expect("run finish records");
    }

    let run_dir = root.join("runs/run_trace");
    let check = check_trace_run(&run_dir).expect("trace check runs");
    assert_eq!(check["ok"], true);
    let events = load_trace_events(run_dir.join("trace.jsonl")).expect("trace events load");
    assert_eq!(events[3]["attributes"]["api_key"], "[redacted]");
    let summary = load_trace_summary(run_dir.join("summary.json")).expect("summary loads");
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["model_call_count"], 1);
    assert_eq!(summary["total_input_tokens"], 3);
    assert_eq!(metadata["agent_trace"]["run_id"], "run_trace");

    fs::remove_dir_all(root).expect("temporary trace root is removed");
}
