pub(super) use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) use openagent_protocol::{ChatMessage, Role, Usage};
pub(super) use openagent_protocol::{
    MessagePartKind, MessageStatus, message_parts_to_chat_messages,
};
pub(super) use openagent_session::{
    AgentTraceRecorder, FileSessionStore, ObservationConfig, ObservationEvent,
    ObservationEventOptions, ObservationRecorder, ObservationTraceRecord, RunRecord,
    RuntimeLogRecord, RuntimeLogger, RuntimeLoggingConfig, RuntimeWarningConfig,
    RuntimeWarningRecord, Session, SessionEventOptions, SessionPartOptions, SessionStatus,
    StartRunOptions, TodoItem, TraceConfig, TraceEvent, TraceEventOptions, check_trace_run,
    format_runtime_warning_event, input_preview, load_trace_events, load_trace_summary,
    output_stats, render_trace_summary, sanitize_observation_value, sanitize_trace_value,
    step_usage_warnings,
};
use serde::Serialize;
pub(super) use serde_json::{Value, json};

pub(super) fn value(payload: impl Serialize) -> Value {
    serde_json::to_value(payload).expect("payload serializes")
}

pub(super) fn read_json(path: impl Into<PathBuf>) -> Value {
    serde_json::from_str(&fs::read_to_string(path.into()).expect("JSON file reads"))
        .expect("JSON file parses")
}

pub(super) fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after UNIX epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
    fs::create_dir_all(&path).expect("temp dir is created");
    path
}
