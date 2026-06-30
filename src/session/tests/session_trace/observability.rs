use super::common::*;
use super::fixtures::fixture_observation_trace;

#[test]
fn observability_logging_and_warnings_write_sanitized_records() {
    let root = unique_temp_dir("openagent-observability");
    let mut metadata = BTreeMap::new();
    {
        let trace = fixture_observation_trace();
        let mut recorder = ObservationRecorder::new(
            trace,
            Some(ObservationConfig {
                jsonl: true,
                jsonl_dir: "observability".to_string(),
                max_events: 2,
                ..ObservationConfig::default()
            }),
            &root,
            &mut metadata,
        );
        recorder
            .event(
                "tool.call.finished",
                "tool",
                BTreeMap::from([
                    ("token".to_string(), json!("secret")),
                    ("output_lines".to_string(), json!(2)),
                ]),
                ObservationEventOptions {
                    event_id: Some("event_1".to_string()),
                    span_id: Some("span_tool".to_string()),
                    parent_span_id: Some("span_step".to_string()),
                    ..ObservationEventOptions::default()
                },
            )
            .expect("observation records");
    }
    assert_eq!(metadata["observability"]["event_count"], 1);
    assert_eq!(
        metadata["observability"]["events"][0]["attributes"]["token"],
        "[redacted]"
    );

    {
        let mut logger = RuntimeLogger::new(
            "session_fixture",
            &mut metadata,
            Some(RuntimeLoggingConfig {
                jsonl: true,
                jsonl_dir: "logs".to_string(),
                level: "WARNING".to_string(),
                ..RuntimeLoggingConfig::default()
            }),
            &root,
            Some("run_fixture".to_string()),
            Some("trace_fixture".to_string()),
        );
        assert!(
            logger
                .log(
                    "INFO",
                    "ignored",
                    "runtime",
                    BTreeMap::new(),
                    Some(1_781_840_000_500),
                )
                .expect("info log filters")
                .is_none()
        );
        logger
            .log(
                "WARNING",
                "Tool output was truncated.",
                "tool",
                BTreeMap::from([("authorization".to_string(), json!("secret"))]),
                Some(1_781_840_000_501),
            )
            .expect("warning log records");
    }
    assert_eq!(metadata["runtime_logging"]["record_count"], 1);
    assert_eq!(
        metadata["runtime_logging"]["records"][0]["attributes"]["authorization"],
        "[redacted]"
    );

    let warnings = step_usage_warnings(
        &RuntimeWarningConfig {
            enabled: true,
            max_step_total_tokens: Some(9),
            ..RuntimeWarningConfig::default()
        },
        &Usage {
            input_tokens: 5,
            output_tokens: 7,
            cost: 0.02,
        },
        1,
    );
    assert_eq!(warnings.len(), 1);
    let event = warnings[0].to_event();
    assert_eq!(event["display"]["title"], "Step token budget exceeded");
    assert!(
        format_runtime_warning_event(&event)
            .expect("warning formats")
            .contains("total_tokens=12")
    );

    fs::remove_dir_all(root).expect("temporary observability root is removed");
}
