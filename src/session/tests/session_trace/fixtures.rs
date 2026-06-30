use super::common::*;

pub(super) fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/golden/rust_rewrite/session_trace_observability.json"
    ))
    .expect("fixture JSON parses")
}

pub(super) fn fixture_todo() -> TodoItem {
    TodoItem::new("port session store", "in_progress", "high", "todo-fixture")
}

pub(super) fn fixture_message() -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: "Remember this fixture.".to_string(),
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::from([("message_id".to_string(), json!("msg_fixture"))]),
    }
}

pub(super) fn fixture_session_event() -> Value {
    json!({
        "schema_version": "openagent.session_event.v1",
        "seq": 1,
        "event": "model.usage",
        "timestamp_ms": 1781840000100_u64,
        "session_id": "session_fixture",
        "run_id": "run_fixture",
        "kind": "model",
        "status": "ok",
        "duration_ms": 12,
        "attributes": {
            "input_tokens": 11,
            "output_tokens": 7,
            "cost": 0.001,
            "authorization": "secret",
        },
    })
}

pub(super) fn fixture_session_part() -> Value {
    json!({
        "schema_version": "openagent.session_part.v1",
        "part_id": "part_fixture",
        "seq": 1,
        "type": "usage",
        "timestamp_ms": 1781840000110_u64,
        "session_id": "session_fixture",
        "run_id": "run_fixture",
        "step_index": 1,
        "status": "ok",
        "attributes": {"input_tokens": 11, "output_tokens": 7},
    })
}

pub(super) fn fixture_session_state() -> Value {
    json!({
        "schema_version": "openagent.session_state.v1",
        "session_id": "session_fixture",
        "run_id": "run_fixture",
        "workspace": "/tmp/openagent-fixture",
        "status": "idle",
        "updated_at_ms": 1781840000120_u64,
        "messages": [{
            "message_id": "msg_fixture",
            "index": 0,
            "role": "user",
            "content": "Remember this fixture.",
            "name": Value::Null,
            "tool_call_id": Value::Null,
            "metadata": {"message_id": "msg_fixture"},
        }],
        "todos": [fixture_todo()],
        "metadata": {
            "session_store": {
                "enabled": true,
                "type": "file",
                "root_dir": "/tmp/openagent-fixture/.openagent/sessions",
                "session_id": "session_fixture",
                "run_id": "run_fixture",
            }
        },
    })
}

pub(super) fn fixture_session_summary() -> Value {
    json!({
        "schema_version": "openagent.run_summary.v1",
        "session_id": "session_fixture",
        "run_id": "run_fixture",
        "event_count": 2,
        "part_count": 1,
        "part_type_counts": {"usage": 1},
        "message_count": 1,
        "step_count": 0,
        "tool_call_count": 0,
        "runtime_warning_count": 0,
        "patch_count": 0,
        "total_input_tokens": 11,
        "total_output_tokens": 7,
        "total_cost": 0.001,
        "status": "completed",
    })
}

pub(super) fn fixture_trace_config() -> TraceConfig {
    TraceConfig {
        root_dir: "runs".to_string(),
        max_events: 12,
        exporters: BTreeMap::from([("langfuse".to_string(), json!({"enabled": false}))]),
        ..TraceConfig::default()
    }
}

pub(super) fn fixture_run_record() -> RunRecord {
    RunRecord {
        run_id: "run_fixture".to_string(),
        trace_id: "trace_fixture".to_string(),
        session_id: "session_fixture".to_string(),
        agent_name: "fixture-agent".to_string(),
        model_id: Some("fixture-model".to_string()),
        provider_id: Some("fixture-provider".to_string()),
        workspace: Some("/tmp/openagent-fixture".to_string()),
        started_at_ms: 1_781_840_000_000,
    }
}

pub(super) fn fixture_trace_event() -> TraceEvent {
    let attrs = sanitize_trace_value(json!({
        "api_key": "secret",
        "input_tokens": 11,
        "output_tokens": 7,
        "cost": 0.001,
        "prompt": "P".repeat(4100),
    }));
    TraceEvent {
        seq: 1,
        event: "model.call.finished".to_string(),
        event_id: Some("event_fixture".to_string()),
        timestamp_ms: 1_781_840_000_200,
        run_id: "run_fixture".to_string(),
        trace_id: "trace_fixture".to_string(),
        session_id: "session_fixture".to_string(),
        kind: "model".to_string(),
        status: "ok".to_string(),
        span_id: Some("span_model".to_string()),
        parent_span_id: Some("span_step".to_string()),
        duration_ms: Some(25),
        attributes: attrs
            .as_object()
            .expect("attrs object")
            .clone()
            .into_iter()
            .collect(),
    }
}

pub(super) fn fixture_observation_config() -> ObservationConfig {
    ObservationConfig {
        jsonl: true,
        jsonl_dir: "observability".to_string(),
        max_events: 3,
        ..ObservationConfig::default()
    }
}

pub(super) fn fixture_observation_trace() -> ObservationTraceRecord {
    ObservationTraceRecord {
        trace_id: "trace_fixture".to_string(),
        session_id: "session_fixture".to_string(),
        run_id: "run_fixture".to_string(),
        agent_name: "fixture-agent".to_string(),
        model_id: Some("fixture-model".to_string()),
        provider_id: Some("fixture-provider".to_string()),
        workspace: Some("/tmp/openagent-fixture".to_string()),
        started_at_ms: 1_781_840_000_000,
    }
}

pub(super) fn fixture_observation_event() -> ObservationEvent {
    let attrs = sanitize_observation_value(json!({
        "token": "secret",
        "output_lines": 2,
        "result_summary": "ok",
    }));
    ObservationEvent {
        event_id: "event_observation".to_string(),
        trace_id: "trace_fixture".to_string(),
        run_id: "run_fixture".to_string(),
        session_id: "session_fixture".to_string(),
        span_id: Some("span_tool".to_string()),
        parent_span_id: Some("span_step".to_string()),
        name: "tool.call.finished".to_string(),
        kind: "tool".to_string(),
        timestamp_ms: 1_781_840_000_400,
        duration_ms: Some(9),
        status: "ok".to_string(),
        attributes: attrs
            .as_object()
            .expect("attrs object")
            .clone()
            .into_iter()
            .collect(),
    }
}

pub(super) fn fixture_logging_config() -> RuntimeLoggingConfig {
    RuntimeLoggingConfig {
        jsonl: true,
        jsonl_dir: "logs".to_string(),
        level: "WARNING".to_string(),
        structured_logging: false,
        ..RuntimeLoggingConfig::default()
    }
}

pub(super) fn fixture_log_record() -> RuntimeLogRecord {
    let attrs = sanitize_observation_value(json!({"authorization": "secret", "output_lines": 2}));
    RuntimeLogRecord {
        log_id: "log_fixture".to_string(),
        timestamp_ms: 1_781_840_000_500,
        level: "WARNING".to_string(),
        message: "Tool output was truncated.".to_string(),
        category: "tool".to_string(),
        session_id: "session_fixture".to_string(),
        run_id: Some("run_fixture".to_string()),
        trace_id: Some("trace_fixture".to_string()),
        span_id: Some("span_tool".to_string()),
        attributes: attrs
            .as_object()
            .expect("attrs object")
            .clone()
            .into_iter()
            .collect(),
    }
}

pub(super) fn fixture_warning_config() -> RuntimeWarningConfig {
    RuntimeWarningConfig {
        enabled: true,
        max_step_total_tokens: Some(12),
        ..RuntimeWarningConfig::default()
    }
}

pub(super) fn fixture_warning_record() -> RuntimeWarningRecord {
    RuntimeWarningRecord {
        code: "step_total_tokens_exceeded".to_string(),
        severity: "warning".to_string(),
        message: "Step total tokens exceeded budget: 18 > 12.".to_string(),
        metrics: BTreeMap::from([
            ("step_index".to_string(), json!(1)),
            ("input_tokens".to_string(), json!(11)),
            ("output_tokens".to_string(), json!(7)),
            ("total_tokens".to_string(), json!(18)),
            ("threshold".to_string(), json!(12)),
        ]),
    }
}
