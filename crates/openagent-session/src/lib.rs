//! Session, trace, and observability crate for the Rust rewrite.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use openagent_protocol::{ChatMessage, Role, Usage};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const SESSION_STORE_METADATA_KEY: &str = "session_store";
pub const TRACE_METADATA_KEY: &str = "agent_trace";
pub const OBSERVABILITY_METADATA_KEY: &str = "observability";
pub const LOGGING_METADATA_KEY: &str = "runtime_logging";
pub const DEFAULT_SESSION_STORE_ROOT: &str = ".openagent/sessions";
pub const DEFAULT_TRACE_ROOT: &str = ".openagent/runs";
pub const DEFAULT_OBSERVABILITY_JSONL_DIR: &str = ".openagent/observability";
pub const DEFAULT_LOGGING_JSONL_DIR: &str = ".openagent/logs";

const DEFAULT_MAX_EVENTS: u64 = 500;
const DEFAULT_TRACE_MAX_EVENTS: u64 = 2000;
const DEFAULT_INPUT_PREVIEW_CHARS: usize = 2048;
const DEFAULT_FIELD_PREVIEW_CHARS: usize = 4096;
const SENSITIVE_KEY_MARKERS: &[&str] = &[
    "api_key",
    "apikey",
    "authorization",
    "cookie",
    "password",
    "secret",
    "token",
];
const SAFE_TOKEN_METRIC_KEYS: &[&str] = &[
    "estimated_input_tokens",
    "input_limit_tokens",
    "input_tokens",
    "max_output_tokens",
    "output_tokens",
    "reserved_output_tokens",
];

pub type SessionError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type SessionResult<T> = Result<T, SessionError>;

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn protocol_crate_name() -> &'static str {
    openagent_protocol::crate_name()
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    #[default]
    Idle,
    Running,
    Paused,
    Stop,
    Compacting,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    pub priority: String,
    pub id: String,
}

impl TodoItem {
    #[must_use]
    pub fn new(
        content: impl Into<String>,
        status: impl Into<String>,
        priority: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            status: status.into(),
            priority: priority.into(),
            id: id.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: String,
    pub directory: PathBuf,
    pub status: SessionStatus,
    pub messages: Vec<ChatMessage>,
    pub todos: Vec<TodoItem>,
    pub metadata: BTreeMap<String, Value>,
}

impl Session {
    #[must_use]
    pub fn new(id: impl Into<String>, directory: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            directory: directory.into(),
            status: SessionStatus::Idle,
            messages: Vec::new(),
            todos: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, message: ChatMessage) {
        self.messages.push(message);
    }

    pub fn set_todos(&mut self, todos: Vec<TodoItem>) {
        self.todos = todos;
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionEventRecord {
    pub schema_version: String,
    pub seq: u64,
    pub event: String,
    pub timestamp_ms: u64,
    pub session_id: String,
    pub run_id: String,
    pub kind: String,
    pub status: String,
    pub duration_ms: Option<u64>,
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionPartRecord {
    pub schema_version: String,
    pub part_id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub part_type: String,
    pub timestamp_ms: u64,
    pub session_id: String,
    pub run_id: String,
    pub step_index: Option<u64>,
    pub status: String,
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StoredMessage {
    pub message_id: String,
    pub index: u64,
    pub role: Role,
    pub content: String,
    pub name: Option<String>,
    pub tool_call_id: Option<String>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionStateRecord {
    pub schema_version: String,
    pub session_id: String,
    pub run_id: Option<String>,
    pub workspace: String,
    pub status: SessionStatus,
    pub updated_at_ms: u64,
    pub messages: Vec<StoredMessage>,
    pub todos: Vec<TodoItem>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunSummaryRecord {
    pub schema_version: String,
    pub session_id: String,
    pub run_id: String,
    pub event_count: u64,
    pub part_count: u64,
    pub part_type_counts: BTreeMap<String, u64>,
    pub message_count: u64,
    pub step_count: u64,
    pub tool_call_count: u64,
    pub runtime_warning_count: u64,
    pub patch_count: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost: f64,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionStoreMetadata {
    pub enabled: bool,
    #[serde(rename = "type")]
    pub store_type: String,
    pub root_dir: String,
    pub session_id: String,
    pub run_id: String,
    pub session_dir: String,
    pub ledger_path: String,
    pub transcript_path: String,
    pub state_path: String,
    pub run_dir: String,
    pub parts_path: String,
}

#[derive(Clone, Debug)]
pub struct StartRunOptions {
    pub run_id: String,
    pub trace_id: String,
    pub agent_name: String,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
    pub permission: String,
    pub max_steps: u64,
    pub started_at_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct FileSessionStore {
    pub root: PathBuf,
}

impl FileSessionStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn from_options(options: Option<&Value>, base_dir: Option<&Path>) -> Option<Self> {
        let raw = options
            .and_then(|value| value.get(SESSION_STORE_METADATA_KEY))
            .cloned()
            .unwrap_or_else(|| json!({}));
        if raw == Value::Bool(false) {
            return None;
        }
        let object = raw.as_object();
        if object
            .and_then(|items| items.get("enabled"))
            .is_some_and(|value| !bool_option(value, true))
        {
            return None;
        }
        let root_raw = object
            .and_then(|items| items.get("root_dir"))
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_SESSION_STORE_ROOT);
        let mut root = PathBuf::from(root_raw);
        if !root.is_absolute() {
            root = base_dir.unwrap_or_else(|| Path::new(".")).join(root);
        }
        Some(Self::new(root))
    }

    pub fn start_run(
        &self,
        session: &mut Session,
        options: StartRunOptions,
    ) -> SessionResult<SessionStoreMetadata> {
        let started = options.started_at_ms.unwrap_or_else(now_ms);
        fs::create_dir_all(self.session_dir(&session.id))?;
        fs::create_dir_all(self.run_dir(&session.id, &options.run_id))?;
        write_json(
            &self.session_json_path(&session.id),
            &json!({
                "schema_version": "openagent.session.v1",
                "session_id": session.id,
                "workspace": session.directory.to_string_lossy(),
                "status": session_status_str(&session.status),
                "created_at_ms": started,
                "updated_at_ms": started,
                "active_run_id": options.run_id,
            }),
        )?;
        write_json(
            &self.run_json_path(&session.id, &options.run_id),
            &json!({
                "schema_version": "openagent.run.v1",
                "session_id": session.id,
                "run_id": options.run_id,
                "trace_id": options.trace_id,
                "agent_name": options.agent_name,
                "model_id": options.model_id,
                "provider_id": options.provider_id,
                "permission": options.permission,
                "max_steps": options.max_steps,
                "status": "running",
                "started_at_ms": started,
                "ended_at_ms": Value::Null,
            }),
        )?;
        let metadata = self.metadata(&session.id, &options.run_id);
        session.metadata.insert(
            SESSION_STORE_METADATA_KEY.to_string(),
            serde_json::to_value(&metadata)?,
        );
        append_jsonl(
            &self.index_path(),
            &json!({"event": "run.started", "session_id": session.id, "run_id": options.run_id, "timestamp_ms": started}),
        )?;
        self.record_event(
            &session.id,
            &options.run_id,
            "run.started",
            SessionEventOptions {
                kind: "run".to_string(),
                attributes: BTreeMap::from([
                    ("agent_name".to_string(), json!(options.agent_name)),
                    ("model_id".to_string(), json!(options.model_id)),
                    ("provider_id".to_string(), json!(options.provider_id)),
                    ("permission".to_string(), json!(options.permission)),
                    ("max_steps".to_string(), json!(options.max_steps)),
                ]),
                ..SessionEventOptions::default()
            },
        )?;
        self.save_state(session, Some(&options.run_id))?;
        Ok(metadata)
    }

    pub fn append_message(
        &self,
        session: &Session,
        message: &ChatMessage,
        run_id: &str,
        index: u64,
    ) -> SessionResult<()> {
        let message_id = message_id(message);
        append_jsonl(
            &self.transcript_path(&session.id),
            &json!({
                "schema_version": "openagent.message.v1",
                "message_id": message_id,
                "session_id": session.id,
                "run_id": run_id,
                "index": index,
                "role": message.role,
                "content": message.content,
                "name": message.name,
                "tool_call_id": message.tool_call_id,
                "metadata": message.metadata,
                "timestamp_ms": now_ms(),
            }),
        )?;
        self.record_event(
            &session.id,
            run_id,
            "message.appended",
            SessionEventOptions {
                kind: "message".to_string(),
                attributes: BTreeMap::from([
                    ("message_id".to_string(), json!(message_id)),
                    ("index".to_string(), json!(index)),
                    ("role".to_string(), json!(message.role)),
                    (
                        "content_chars".to_string(),
                        json!(message.content.chars().count()),
                    ),
                    ("tool_call_id".to_string(), json!(message.tool_call_id)),
                ]),
                ..SessionEventOptions::default()
            },
        )?;
        Ok(())
    }

    pub fn record_event(
        &self,
        session_id: &str,
        run_id: &str,
        event: &str,
        options: SessionEventOptions,
    ) -> SessionResult<SessionEventRecord> {
        let event_path = self.events_path(session_id, run_id);
        let payload = SessionEventRecord {
            schema_version: "openagent.session_event.v1".to_string(),
            seq: next_seq(&event_path)?,
            event: event.to_string(),
            timestamp_ms: options.timestamp_ms.unwrap_or_else(now_ms),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            kind: options.kind,
            status: if options.status == "error" {
                "error"
            } else {
                "ok"
            }
            .to_string(),
            duration_ms: options.duration_ms,
            attributes: options.attributes,
        };
        append_jsonl(&event_path, &payload)?;
        self.write_run_summary(session_id, run_id)?;
        Ok(payload)
    }

    pub fn append_part(
        &self,
        session_id: &str,
        run_id: &str,
        part_type: &str,
        options: SessionPartOptions,
    ) -> SessionResult<SessionPartRecord> {
        let parts_path = self.parts_path(session_id, run_id);
        let payload = SessionPartRecord {
            schema_version: "openagent.session_part.v1".to_string(),
            part_id: options.part_id.unwrap_or_else(|| new_id("part")),
            seq: next_seq(&parts_path)?,
            part_type: part_type.to_string(),
            timestamp_ms: options.timestamp_ms.unwrap_or_else(now_ms),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            step_index: options.step_index,
            status: if options.status == "error" {
                "error"
            } else {
                "ok"
            }
            .to_string(),
            attributes: options.attributes,
        };
        append_jsonl(&parts_path, &payload)?;
        self.write_run_summary(session_id, run_id)?;
        Ok(payload)
    }

    pub fn finish_run(
        &self,
        session: &Session,
        run_id: &str,
        status: &str,
        steps: u64,
        finish_reason: Option<&str>,
        error: Option<&str>,
    ) -> SessionResult<()> {
        let ended = now_ms();
        self.record_event(
            &session.id,
            run_id,
            if status == "completed" {
                "run.finished"
            } else {
                "run.failed"
            },
            SessionEventOptions {
                kind: "run".to_string(),
                status: if status == "completed" { "ok" } else { "error" }.to_string(),
                attributes: BTreeMap::from([
                    ("status".to_string(), json!(status)),
                    ("steps".to_string(), json!(steps)),
                    ("finish_reason".to_string(), json!(finish_reason)),
                    ("error".to_string(), json!(error)),
                ]),
                ..SessionEventOptions::default()
            },
        )?;
        let run_path = self.run_json_path(&session.id, run_id);
        let mut run_record = read_json_object(&run_path)?.unwrap_or_default();
        run_record.insert("status".to_string(), json!(status));
        run_record.insert("ended_at_ms".to_string(), json!(ended));
        run_record.insert("steps".to_string(), json!(steps));
        run_record.insert("finish_reason".to_string(), json!(finish_reason));
        run_record.insert("error".to_string(), json!(error));
        let started = run_record
            .get("started_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or(ended);
        run_record.insert(
            "duration_ms".to_string(),
            json!(ended.saturating_sub(started)),
        );
        write_json(&run_path, &Value::Object(run_record))?;
        self.save_state(session, Some(run_id))
    }

    pub fn save_state(&self, session: &Session, run_id: Option<&str>) -> SessionResult<()> {
        let state = SessionStateRecord {
            schema_version: "openagent.session_state.v1".to_string(),
            session_id: session.id.clone(),
            run_id: run_id.map(ToString::to_string),
            workspace: session.directory.to_string_lossy().to_string(),
            status: session.status.clone(),
            updated_at_ms: now_ms(),
            messages: session
                .messages
                .iter()
                .enumerate()
                .map(|(index, message)| stored_message(message, index as u64))
                .collect(),
            todos: session.todos.clone(),
            metadata: session.metadata.clone(),
        };
        write_json(&self.state_path(&session.id), &state)
    }

    pub fn load_session(&self, session_id: &str) -> SessionResult<Session> {
        let state = if let Some(state) = read_json_object(&self.state_path(session_id))? {
            state
        } else {
            self.reconstruct_state_from_transcript(session_id)?
                .ok_or_else(|| format!("Session state not found: {session_id}"))?
        };
        let messages = state
            .get("messages")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| serde_json::from_value::<ChatMessage>(item.clone()).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let todos = state
            .get("todos")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| serde_json::from_value::<TodoItem>(item.clone()).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let metadata = state
            .get("metadata")
            .and_then(Value::as_object)
            .map(|items| items.clone().into_iter().collect())
            .unwrap_or_default();
        Ok(Session {
            id: state
                .get("session_id")
                .and_then(Value::as_str)
                .unwrap_or(session_id)
                .to_string(),
            directory: PathBuf::from(
                state
                    .get("workspace")
                    .and_then(Value::as_str)
                    .unwrap_or("."),
            ),
            status: session_status_from_value(state.get("status")),
            messages,
            todos,
            metadata,
        })
    }

    pub fn load_parts(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> SessionResult<Vec<SessionPartRecord>> {
        read_jsonl(&self.parts_path(session_id, run_id))?
            .into_iter()
            .map(|value| serde_json::from_value::<SessionPartRecord>(value).map_err(Into::into))
            .collect()
    }

    pub fn write_run_summary(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> SessionResult<RunSummaryRecord> {
        let events = read_jsonl(&self.events_path(session_id, run_id))?;
        let parts = read_jsonl(&self.parts_path(session_id, run_id))?;
        let mut summary = RunSummaryRecord {
            schema_version: "openagent.run_summary.v1".to_string(),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            event_count: events.len() as u64,
            part_count: parts.len() as u64,
            part_type_counts: count_by_key(&parts, "type"),
            message_count: count_events(&events, "message.appended"),
            step_count: count_events(&events, "step.finished"),
            tool_call_count: events
                .iter()
                .filter(|event| {
                    matches!(
                        event.get("event").and_then(Value::as_str),
                        Some("tool.call.finished" | "tool.call.failed")
                    )
                })
                .count() as u64,
            runtime_warning_count: count_events(&events, "runtime.warning"),
            patch_count: count_events(&events, "patch.detected"),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            status: "running".to_string(),
        };
        for event in &events {
            let attrs = event
                .get("attributes")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            match event.get("event").and_then(Value::as_str) {
                Some("model.usage") => {
                    summary.total_input_tokens += attrs
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    summary.total_output_tokens += attrs
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    summary.total_cost += attrs
                        .get("cost")
                        .and_then(Value::as_f64)
                        .unwrap_or_default();
                }
                Some("run.finished") => {
                    summary.status = attrs
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("completed")
                        .to_string();
                }
                Some("run.failed") => {
                    summary.status = attrs
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("failed")
                        .to_string();
                }
                _ => {}
            }
        }
        write_json(&self.summary_path(session_id, run_id), &summary)?;
        Ok(summary)
    }

    #[must_use]
    pub fn metadata(&self, session_id: &str, run_id: &str) -> SessionStoreMetadata {
        SessionStoreMetadata {
            enabled: true,
            store_type: "file".to_string(),
            root_dir: self.root.to_string_lossy().to_string(),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            session_dir: self.session_dir(session_id).to_string_lossy().to_string(),
            ledger_path: self
                .events_path(session_id, run_id)
                .to_string_lossy()
                .to_string(),
            transcript_path: self
                .transcript_path(session_id)
                .to_string_lossy()
                .to_string(),
            state_path: self.state_path(session_id).to_string_lossy().to_string(),
            run_dir: self
                .run_dir(session_id, run_id)
                .to_string_lossy()
                .to_string(),
            parts_path: self
                .parts_path(session_id, run_id)
                .to_string_lossy()
                .to_string(),
        }
    }

    fn reconstruct_state_from_transcript(
        &self,
        session_id: &str,
    ) -> SessionResult<Option<Map<String, Value>>> {
        let transcript_path = self.transcript_path(session_id);
        if !transcript_path.exists() {
            return Ok(None);
        }
        let session_record =
            read_json_object(&self.session_json_path(session_id))?.unwrap_or_default();
        let messages = read_jsonl(&transcript_path)?;
        Ok(Some(Map::from_iter([
            ("session_id".to_string(), json!(session_id)),
            (
                "workspace".to_string(),
                session_record
                    .get("workspace")
                    .cloned()
                    .unwrap_or_else(|| json!(".")),
            ),
            (
                "status".to_string(),
                session_record
                    .get("status")
                    .cloned()
                    .unwrap_or_else(|| json!("idle")),
            ),
            ("messages".to_string(), Value::Array(messages)),
            ("todos".to_string(), json!([])),
            ("metadata".to_string(), json!({})),
        ])))
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(session_id)
    }

    fn run_dir(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.session_dir(session_id).join("runs").join(run_id)
    }

    fn session_json_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("session.json")
    }

    fn transcript_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("transcript.jsonl")
    }

    fn state_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("state.latest.json")
    }

    fn run_json_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("run.json")
    }

    fn events_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("events.jsonl")
    }

    fn parts_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("parts.jsonl")
    }

    fn summary_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("summary.json")
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.jsonl")
    }
}

#[derive(Clone, Debug)]
pub struct SessionEventOptions {
    pub kind: String,
    pub status: String,
    pub attributes: BTreeMap<String, Value>,
    pub duration_ms: Option<u64>,
    pub timestamp_ms: Option<u64>,
}

impl Default for SessionEventOptions {
    fn default() -> Self {
        Self {
            kind: "event".to_string(),
            status: "ok".to_string(),
            attributes: BTreeMap::new(),
            duration_ms: None,
            timestamp_ms: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionPartOptions {
    pub part_id: Option<String>,
    pub attributes: BTreeMap<String, Value>,
    pub step_index: Option<u64>,
    pub status: String,
    pub timestamp_ms: Option<u64>,
}

impl Default for SessionPartOptions {
    fn default() -> Self {
        Self {
            part_id: None,
            attributes: BTreeMap::new(),
            step_index: None,
            status: "ok".to_string(),
            timestamp_ms: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TraceConfig {
    pub enabled: bool,
    pub root_dir: String,
    pub keep_events: bool,
    pub max_events: u64,
    pub write_summary: bool,
    pub exporters: BTreeMap<String, Value>,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            root_dir: DEFAULT_TRACE_ROOT.to_string(),
            keep_events: true,
            max_events: DEFAULT_TRACE_MAX_EVENTS,
            write_summary: true,
            exporters: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunRecord {
    pub run_id: String,
    pub trace_id: String,
    pub session_id: String,
    pub agent_name: String,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
    pub workspace: Option<String>,
    pub started_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TraceEvent {
    pub seq: u64,
    pub event: String,
    pub timestamp_ms: u64,
    pub run_id: String,
    pub trace_id: String,
    pub session_id: String,
    pub event_id: Option<String>,
    pub kind: String,
    pub status: String,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub duration_ms: Option<u64>,
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default)]
pub struct TraceEventOptions {
    pub event_id: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub duration_ms: Option<u64>,
    pub timestamp_ms: Option<u64>,
    pub attributes: BTreeMap<String, Value>,
}

pub struct AgentTraceRecorder<'a> {
    pub run: RunRecord,
    pub config: TraceConfig,
    pub base_dir: PathBuf,
    session_metadata: Option<&'a mut BTreeMap<String, Value>>,
    seq: u64,
    events: Vec<Value>,
    summary: Map<String, Value>,
    closed: bool,
}

impl<'a> AgentTraceRecorder<'a> {
    pub fn new(
        run: RunRecord,
        config: Option<TraceConfig>,
        base_dir: impl Into<PathBuf>,
        session_metadata: Option<&'a mut BTreeMap<String, Value>>,
    ) -> SessionResult<Self> {
        let mut recorder = Self {
            summary: Map::new(),
            run,
            config: config.unwrap_or_default(),
            base_dir: base_dir.into(),
            session_metadata,
            seq: 0,
            events: Vec::new(),
            closed: false,
        };
        recorder.summary = recorder.empty_summary();
        if recorder.config.enabled {
            fs::create_dir_all(recorder.run_dir())?;
            fs::create_dir_all(recorder.artifacts_dir())?;
            recorder.bind_metadata()?;
            recorder.write_process_note("Trace recorder initialized.")?;
            recorder.write_summary()?;
        }
        Ok(recorder)
    }

    #[must_use]
    pub fn root_dir(&self) -> PathBuf {
        let root = PathBuf::from(&self.config.root_dir);
        if root.is_absolute() {
            root
        } else {
            self.base_dir.join(root)
        }
    }

    #[must_use]
    pub fn run_dir(&self) -> PathBuf {
        self.root_dir().join(&self.run.run_id)
    }

    #[must_use]
    pub fn trace_path(&self) -> PathBuf {
        self.run_dir().join("trace.jsonl")
    }

    #[must_use]
    pub fn summary_path(&self) -> PathBuf {
        self.run_dir().join("summary.json")
    }

    #[must_use]
    pub fn process_path(&self) -> PathBuf {
        self.run_dir().join("process.md")
    }

    #[must_use]
    pub fn artifacts_dir(&self) -> PathBuf {
        self.run_dir().join("artifacts")
    }

    pub fn record_event(
        &mut self,
        event: &str,
        options: TraceEventOptions,
    ) -> SessionResult<Option<TraceEvent>> {
        if !self.config.enabled {
            return Ok(None);
        }
        self.seq += 1;
        let trace_event = TraceEvent {
            seq: self.seq,
            event: event.to_string(),
            timestamp_ms: options.timestamp_ms.unwrap_or_else(now_ms),
            run_id: self.run.run_id.clone(),
            trace_id: self.run.trace_id.clone(),
            session_id: self.run.session_id.clone(),
            event_id: options.event_id,
            kind: options.kind.unwrap_or_else(|| "event".to_string()),
            status: if options.status.as_deref() == Some("error") {
                "error"
            } else {
                "ok"
            }
            .to_string(),
            span_id: options.span_id,
            parent_span_id: options.parent_span_id,
            duration_ms: options.duration_ms,
            attributes: sanitize_value_map(options.attributes, DEFAULT_FIELD_PREVIEW_CHARS),
        };
        append_jsonl(&self.trace_path(), &trace_event)?;
        let event_value = serde_json::to_value(&trace_event)?;
        if self.config.keep_events {
            self.events.push(event_value.clone());
            let max_events = self.config.max_events.max(1) as usize;
            if self.events.len() > max_events {
                self.events = self.events[self.events.len() - max_events..].to_vec();
            }
        }
        self.update_summary(&event_value);
        if self.config.write_summary {
            self.write_summary()?;
        }
        if matches!(event, "run.finished" | "run.failed") {
            self.write_process_note(&format!(
                "Run {} after {} trace events.",
                if event == "run.failed" {
                    "failed"
                } else {
                    "completed"
                },
                self.summary
                    .get("event_count")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
            ))?;
            self.close()?;
        }
        Ok(Some(trace_event))
    }

    pub fn finish_run(
        &mut self,
        status: &str,
        attributes: BTreeMap<String, Value>,
    ) -> SessionResult<Option<TraceEvent>> {
        let mut attrs = attributes;
        attrs
            .entry("status".to_string())
            .or_insert_with(|| json!(status));
        self.record_event(
            "run.finished",
            TraceEventOptions {
                kind: Some("run".to_string()),
                attributes: attrs,
                ..TraceEventOptions::default()
            },
        )
    }

    #[must_use]
    pub fn summary(&self) -> Value {
        Value::Object(self.summary.clone())
    }

    pub fn close(&mut self) -> SessionResult<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.sync_exporter_metadata()
    }

    fn bind_metadata(&mut self) -> SessionResult<()> {
        let payload = json!({
            "run_id": self.run.run_id,
            "trace_id": self.run.trace_id,
            "run_dir": self.run_dir().to_string_lossy(),
            "trace_path": self.trace_path().to_string_lossy(),
            "summary_path": self.summary_path().to_string_lossy(),
            "process_path": self.process_path().to_string_lossy(),
            "exporters": {"enabled": [], "diagnostics": []},
        });
        if let Some(metadata) = self.session_metadata.as_mut() {
            (**metadata).insert(TRACE_METADATA_KEY.to_string(), payload);
        }
        Ok(())
    }

    fn sync_exporter_metadata(&mut self) -> SessionResult<()> {
        if let Some(metadata) = self.session_metadata.as_mut()
            && let Some(Value::Object(root)) = (**metadata).get_mut(TRACE_METADATA_KEY)
        {
            root.insert(
                "exporters".to_string(),
                json!({"enabled": [], "diagnostics": []}),
            );
        }
        Ok(())
    }

    fn empty_summary(&self) -> Map<String, Value> {
        Map::from_iter([
            ("run_id".to_string(), json!(self.run.run_id)),
            ("trace_id".to_string(), json!(self.run.trace_id)),
            ("session_id".to_string(), json!(self.run.session_id)),
            ("agent_name".to_string(), json!(self.run.agent_name)),
            ("model_id".to_string(), json!(self.run.model_id)),
            ("provider_id".to_string(), json!(self.run.provider_id)),
            ("workspace".to_string(), json!(self.run.workspace)),
            ("status".to_string(), json!("running")),
            ("started_at_ms".to_string(), json!(self.run.started_at_ms)),
            ("ended_at_ms".to_string(), Value::Null),
            ("duration_ms".to_string(), Value::Null),
            ("event_count".to_string(), json!(0)),
            ("step_count".to_string(), json!(0)),
            ("model_call_count".to_string(), json!(0)),
            ("tool_call_count".to_string(), json!(0)),
            ("mcp_call_count".to_string(), json!(0)),
            ("skill_call_count".to_string(), json!(0)),
            ("local_tool_call_count".to_string(), json!(0)),
            ("artifact_count".to_string(), json!(0)),
            ("error_count".to_string(), json!(0)),
            ("runtime_warning_count".to_string(), json!(0)),
            ("total_latency_ms".to_string(), json!(0)),
            ("total_input_tokens".to_string(), json!(0)),
            ("total_output_tokens".to_string(), json!(0)),
            ("total_reasoning_tokens".to_string(), json!(0)),
            ("total_cache_read_tokens".to_string(), json!(0)),
            ("total_cache_write_tokens".to_string(), json!(0)),
            ("total_cost".to_string(), json!(0.0)),
            ("errors".to_string(), json!([])),
            (
                "paths".to_string(),
                json!({
                    "run_dir": self.run_dir().to_string_lossy(),
                    "trace": self.trace_path().to_string_lossy(),
                    "summary": self.summary_path().to_string_lossy(),
                    "process": self.process_path().to_string_lossy(),
                    "artifacts": self.artifacts_dir().to_string_lossy(),
                }),
            ),
        ])
    }

    fn update_summary(&mut self, event: &Value) {
        inc_summary_u64(&mut self.summary, "event_count", 1);
        if let Some(duration_ms) = event.get("duration_ms").and_then(Value::as_u64) {
            inc_summary_u64(&mut self.summary, "total_latency_ms", duration_ms);
        }
        let name = event
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let kind = event
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let attrs = event
            .get("attributes")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if event.get("status").and_then(Value::as_str) == Some("error") {
            inc_summary_u64(&mut self.summary, "error_count", 1);
        }
        match name {
            "run.finished" => {
                self.summary.insert(
                    "status".to_string(),
                    attrs
                        .get("status")
                        .cloned()
                        .unwrap_or_else(|| json!("completed")),
                );
                self.summary.insert(
                    "ended_at_ms".to_string(),
                    event.get("timestamp_ms").cloned().unwrap_or(Value::Null),
                );
            }
            "run.failed" => {
                self.summary.insert("status".to_string(), json!("failed"));
                self.summary.insert(
                    "ended_at_ms".to_string(),
                    event.get("timestamp_ms").cloned().unwrap_or(Value::Null),
                );
            }
            "step.finished" => inc_summary_u64(&mut self.summary, "step_count", 1),
            "model.call.finished" => {
                inc_summary_u64(&mut self.summary, "model_call_count", 1);
                inc_summary_u64(
                    &mut self.summary,
                    "total_input_tokens",
                    attrs
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                );
                inc_summary_u64(
                    &mut self.summary,
                    "total_output_tokens",
                    attrs
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                );
                inc_summary_u64(
                    &mut self.summary,
                    "total_reasoning_tokens",
                    attrs
                        .get("reasoning_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                );
                inc_summary_u64(
                    &mut self.summary,
                    "total_cache_read_tokens",
                    attrs
                        .get("cache_read_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                );
                inc_summary_u64(
                    &mut self.summary,
                    "total_cache_write_tokens",
                    attrs
                        .get("cache_write_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                );
                inc_summary_f64(
                    &mut self.summary,
                    "total_cost",
                    attrs
                        .get("cost")
                        .and_then(Value::as_f64)
                        .unwrap_or_default(),
                );
            }
            "tool.call.finished" => {
                inc_summary_u64(&mut self.summary, "tool_call_count", 1);
                match tool_source(&attrs) {
                    "mcp" => inc_summary_u64(&mut self.summary, "mcp_call_count", 1),
                    "skill" => inc_summary_u64(&mut self.summary, "skill_call_count", 1),
                    "local_tool" | "local" => {
                        inc_summary_u64(&mut self.summary, "local_tool_call_count", 1)
                    }
                    _ => {}
                }
                if attrs.get("output_path").is_some() {
                    inc_summary_u64(&mut self.summary, "artifact_count", 1);
                }
            }
            "artifact.created" => inc_summary_u64(&mut self.summary, "artifact_count", 1),
            "runtime.warning" => inc_summary_u64(&mut self.summary, "runtime_warning_count", 1),
            _ if kind == "artifact" => inc_summary_u64(&mut self.summary, "artifact_count", 1),
            _ => {}
        }
        let ended_at = self
            .summary
            .get("ended_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or_else(now_ms);
        let started_at = self
            .summary
            .get("started_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or(ended_at);
        self.summary.insert(
            "duration_ms".to_string(),
            json!(ended_at.saturating_sub(started_at)),
        );
    }

    fn write_summary(&self) -> SessionResult<()> {
        write_json(&self.summary_path(), &Value::Object(self.summary.clone()))
    }

    fn write_process_note(&self, message: &str) -> SessionResult<()> {
        let path = self.process_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let existing = if path.exists() {
            fs::read_to_string(&path)?
        } else {
            "# Trace Process\n\n".to_string()
        };
        fs::write(
            path,
            format!("{}\n- {}: {}\n", existing.trim_end(), now_ms(), message),
        )?;
        Ok(())
    }
}

#[must_use]
pub fn load_trace_config(options: Option<&Value>) -> TraceConfig {
    let raw = options
        .and_then(|value| value.get("trace"))
        .and_then(Value::as_object);
    TraceConfig {
        enabled: raw
            .and_then(|items| items.get("enabled"))
            .is_none_or(|value| bool_option(value, true)),
        root_dir: raw
            .and_then(|items| items.get("root_dir").or_else(|| items.get("jsonl_dir")))
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_TRACE_ROOT)
            .to_string(),
        keep_events: raw
            .and_then(|items| items.get("keep_events"))
            .is_none_or(|value| bool_option(value, true)),
        max_events: raw
            .and_then(|items| items.get("max_events"))
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_TRACE_MAX_EVENTS),
        write_summary: raw
            .and_then(|items| items.get("write_summary"))
            .is_none_or(|value| bool_option(value, true)),
        exporters: raw
            .and_then(|items| items.get("exporters"))
            .and_then(Value::as_object)
            .map(|items| items.clone().into_iter().collect())
            .unwrap_or_default(),
    }
}

pub fn load_trace_events(path: impl AsRef<Path>) -> SessionResult<Vec<Value>> {
    read_jsonl(path.as_ref())
}

pub fn load_trace_summary(path: impl AsRef<Path>) -> SessionResult<Value> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

#[must_use]
pub fn render_trace_summary(summary: &Value) -> String {
    let cost = summary
        .get("total_cost")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    format!(
        "Run: {}\nStatus: {}\nDuration: {}ms\nEvents: {}\nSteps: {}\nModel calls: {}\nTool calls: {}\nMCP calls: {}\nSkill calls: {}\nTokens: {}/{}\nReasoning tokens: {}\nCache tokens: read={} write={}\nCost: {:.6}\nErrors: {}\n",
        string_field(summary, "run_id"),
        string_field(summary, "status"),
        u64_field(summary, "duration_ms"),
        u64_field(summary, "event_count"),
        u64_field(summary, "step_count"),
        u64_field(summary, "model_call_count"),
        u64_field(summary, "tool_call_count"),
        u64_field(summary, "mcp_call_count"),
        u64_field(summary, "skill_call_count"),
        u64_field(summary, "total_input_tokens"),
        u64_field(summary, "total_output_tokens"),
        u64_field(summary, "total_reasoning_tokens"),
        u64_field(summary, "total_cache_read_tokens"),
        u64_field(summary, "total_cache_write_tokens"),
        cost,
        u64_field(summary, "error_count"),
    )
}

pub fn check_trace_run(run_dir: impl AsRef<Path>) -> SessionResult<Value> {
    let run_path = run_dir.as_ref();
    let trace_path = run_path.join("trace.jsonl");
    let summary_path = run_path.join("summary.json");
    let mut errors = Vec::new();
    let events = if trace_path.exists() {
        read_jsonl(&trace_path)?
    } else {
        errors.push("missing trace.jsonl".to_string());
        Vec::new()
    };
    let summary = if summary_path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&summary_path)?)?
    } else {
        errors.push("missing summary.json".to_string());
        json!({})
    };
    let names = events
        .iter()
        .filter_map(|event| event.get("event").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let seqs = events
        .iter()
        .filter_map(|event| event.get("seq").and_then(Value::as_u64))
        .collect::<Vec<_>>();
    if events.is_empty() {
        errors.push("trace has no events".to_string());
    }
    if !seqs.is_empty() && seqs != (1..=seqs.len() as u64).collect::<Vec<_>>() {
        errors.push("event seq values are not contiguous from 1".to_string());
    }
    if !names.contains(&"run.started") {
        errors.push("missing run.started".to_string());
    }
    if !names
        .iter()
        .any(|name| matches!(*name, "run.finished" | "run.failed"))
    {
        errors.push("missing terminal run event".to_string());
    }
    if !names.contains(&"step.started") {
        errors.push("missing step.started".to_string());
    }
    if !names.contains(&"step.finished") {
        errors.push("missing step.finished".to_string());
    }
    if !names.contains(&"model.call.started") {
        errors.push("missing model.call.started".to_string());
    }
    if !names.contains(&"model.call.finished") {
        errors.push("missing model.call.finished".to_string());
    }
    if summary
        .get("event_count")
        .and_then(Value::as_u64)
        .is_some_and(|count| count != events.len() as u64)
    {
        errors.push("summary event_count does not match trace length".to_string());
    }
    Ok(json!({
        "ok": errors.is_empty(),
        "run_id": summary.get("run_id").cloned().unwrap_or(Value::Null),
        "event_count": events.len(),
        "errors": errors,
    }))
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ObservationConfig {
    pub enabled: bool,
    pub keep_events: bool,
    pub jsonl: bool,
    pub jsonl_dir: String,
    pub max_events: u64,
    pub input_preview_chars: usize,
    pub include_traceback: bool,
}

impl Default for ObservationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_events: true,
            jsonl: false,
            jsonl_dir: DEFAULT_OBSERVABILITY_JSONL_DIR.to_string(),
            max_events: DEFAULT_MAX_EVENTS,
            input_preview_chars: DEFAULT_INPUT_PREVIEW_CHARS,
            include_traceback: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ObservationTraceRecord {
    pub trace_id: String,
    pub session_id: String,
    pub run_id: String,
    pub agent_name: String,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
    pub workspace: Option<String>,
    pub started_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ObservationEvent {
    pub event_id: String,
    pub trace_id: String,
    pub run_id: String,
    pub session_id: String,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub timestamp_ms: u64,
    pub duration_ms: Option<u64>,
    pub status: String,
    pub attributes: BTreeMap<String, Value>,
}

pub struct ObservationRecorder<'a> {
    pub trace: ObservationTraceRecord,
    pub config: ObservationConfig,
    pub base_dir: PathBuf,
    session_metadata: &'a mut BTreeMap<String, Value>,
}

impl<'a> ObservationRecorder<'a> {
    pub fn new(
        trace: ObservationTraceRecord,
        config: Option<ObservationConfig>,
        base_dir: impl Into<PathBuf>,
        session_metadata: &'a mut BTreeMap<String, Value>,
    ) -> Self {
        let mut recorder = Self {
            trace,
            config: config.unwrap_or_default(),
            base_dir: base_dir.into(),
            session_metadata,
        };
        if recorder.config.enabled {
            recorder.ensure_metadata_root();
        }
        recorder
    }

    pub fn event(
        &mut self,
        name: &str,
        kind: &str,
        attributes: BTreeMap<String, Value>,
        options: ObservationEventOptions,
    ) -> SessionResult<Option<ObservationEvent>> {
        if !self.config.enabled {
            return Ok(None);
        }
        let event = ObservationEvent {
            event_id: options.event_id.unwrap_or_else(|| new_id("event")),
            trace_id: self.trace.trace_id.clone(),
            run_id: self.trace.run_id.clone(),
            session_id: self.trace.session_id.clone(),
            span_id: options.span_id,
            parent_span_id: options.parent_span_id,
            name: name.to_string(),
            kind: kind.to_string(),
            timestamp_ms: options.timestamp_ms.unwrap_or_else(now_ms),
            duration_ms: options.duration_ms,
            status: if options.status == "error" {
                "error"
            } else {
                "ok"
            }
            .to_string(),
            attributes: sanitize_value_map(attributes, DEFAULT_FIELD_PREVIEW_CHARS),
        };
        self.record(&event)?;
        Ok(Some(event))
    }

    fn ensure_metadata_root(&mut self) {
        let jsonl_path = if self.config.jsonl {
            Some(self.jsonl_path().to_string_lossy().to_string())
        } else {
            None
        };
        let root = self
            .session_metadata
            .entry(OBSERVABILITY_METADATA_KEY.to_string())
            .or_insert_with(|| json!({}));
        if !root.is_object() {
            *root = json!({});
        }
        if let Some(object) = root.as_object_mut() {
            object.insert(
                "trace".to_string(),
                serde_json::to_value(&self.trace).unwrap_or(Value::Null),
            );
            object
                .entry("events".to_string())
                .or_insert_with(|| json!([]));
            object
                .entry("event_count".to_string())
                .or_insert_with(|| json!(0));
            object.insert(
                "jsonl_path".to_string(),
                jsonl_path.map_or(Value::Null, Value::String),
            );
        }
    }

    fn record(&mut self, event: &ObservationEvent) -> SessionResult<()> {
        self.ensure_metadata_root();
        if let Some(Value::Object(root)) = self.session_metadata.get_mut(OBSERVABILITY_METADATA_KEY)
        {
            let count = root
                .get("event_count")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                + 1;
            root.insert("event_count".to_string(), json!(count));
            root.insert("last_event_at_ms".to_string(), json!(event.timestamp_ms));
            if self.config.keep_events {
                let mut events = root
                    .get("events")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                events.push(serde_json::to_value(event)?);
                let max_events = self.config.max_events.max(1) as usize;
                if events.len() > max_events {
                    events = events[events.len() - max_events..].to_vec();
                }
                root.insert("events".to_string(), Value::Array(events));
            }
        }
        if self.config.jsonl {
            append_jsonl(&self.jsonl_path(), event)?;
        }
        Ok(())
    }

    fn jsonl_path(&self) -> PathBuf {
        let root = PathBuf::from(&self.config.jsonl_dir);
        let root = if root.is_absolute() {
            root
        } else {
            self.base_dir.join(root)
        };
        root.join(&self.trace.session_id)
            .join(format!("{}.jsonl", self.trace.run_id))
    }
}

#[derive(Clone, Debug)]
pub struct ObservationEventOptions {
    pub event_id: Option<String>,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub duration_ms: Option<u64>,
    pub timestamp_ms: Option<u64>,
    pub status: String,
}

impl Default for ObservationEventOptions {
    fn default() -> Self {
        Self {
            event_id: None,
            span_id: None,
            parent_span_id: None,
            duration_ms: None,
            timestamp_ms: None,
            status: "ok".to_string(),
        }
    }
}

#[must_use]
pub fn load_observation_config(options: Option<&Value>) -> ObservationConfig {
    let raw = options
        .and_then(|value| value.get("observability"))
        .and_then(Value::as_object);
    ObservationConfig {
        enabled: raw
            .and_then(|items| items.get("enabled"))
            .is_none_or(|value| bool_option(value, true)),
        keep_events: raw
            .and_then(|items| items.get("keep_events"))
            .is_none_or(|value| bool_option(value, true)),
        jsonl: raw
            .and_then(|items| items.get("jsonl"))
            .is_some_and(|value| bool_option(value, false)),
        jsonl_dir: raw
            .and_then(|items| items.get("jsonl_dir"))
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_OBSERVABILITY_JSONL_DIR)
            .to_string(),
        max_events: raw
            .and_then(|items| items.get("max_events"))
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_EVENTS),
        input_preview_chars: raw
            .and_then(|items| items.get("input_preview_chars"))
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_INPUT_PREVIEW_CHARS as u64) as usize,
        include_traceback: raw
            .and_then(|items| items.get("include_traceback"))
            .is_some_and(|value| bool_option(value, false)),
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeLoggingConfig {
    pub enabled: bool,
    pub keep_records: bool,
    pub jsonl: bool,
    pub jsonl_dir: String,
    pub max_records: u64,
    pub input_preview_chars: usize,
    pub level: String,
    pub python_logging: bool,
    pub logger_name: String,
    pub include_context: bool,
}

impl Default for RuntimeLoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_records: true,
            jsonl: false,
            jsonl_dir: DEFAULT_LOGGING_JSONL_DIR.to_string(),
            max_records: DEFAULT_MAX_EVENTS,
            input_preview_chars: DEFAULT_INPUT_PREVIEW_CHARS,
            level: "INFO".to_string(),
            python_logging: false,
            logger_name: "openagent.runtime".to_string(),
            include_context: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeLogRecord {
    pub log_id: String,
    pub timestamp_ms: u64,
    pub level: String,
    pub message: String,
    pub category: String,
    pub session_id: String,
    pub run_id: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub attributes: BTreeMap<String, Value>,
}

pub struct RuntimeLogger<'a> {
    session_id: String,
    session_metadata: &'a mut BTreeMap<String, Value>,
    config: RuntimeLoggingConfig,
    base_dir: PathBuf,
    run_id: Option<String>,
    trace_id: Option<String>,
}

impl<'a> RuntimeLogger<'a> {
    pub fn new(
        session_id: impl Into<String>,
        session_metadata: &'a mut BTreeMap<String, Value>,
        config: Option<RuntimeLoggingConfig>,
        base_dir: impl Into<PathBuf>,
        run_id: Option<String>,
        trace_id: Option<String>,
    ) -> Self {
        let mut logger = Self {
            session_id: session_id.into(),
            session_metadata,
            config: config.unwrap_or_default(),
            base_dir: base_dir.into(),
            run_id,
            trace_id,
        };
        if logger.config.enabled {
            logger.ensure_metadata_root();
        }
        logger
    }

    pub fn log(
        &mut self,
        level: &str,
        message: &str,
        category: &str,
        attributes: BTreeMap<String, Value>,
        timestamp_ms: Option<u64>,
    ) -> SessionResult<Option<RuntimeLogRecord>> {
        if !self.config.enabled {
            return Ok(None);
        }
        let normalized_level = normalize_level(level);
        if level_number(&normalized_level) < level_number(&self.config.level) {
            return Ok(None);
        }
        let record = RuntimeLogRecord {
            log_id: new_id("log"),
            timestamp_ms: timestamp_ms.unwrap_or_else(now_ms),
            level: normalized_level,
            message: message.to_string(),
            category: category.to_string(),
            session_id: self.session_id.clone(),
            run_id: self.run_id.clone(),
            trace_id: self.trace_id.clone(),
            span_id: None,
            attributes: sanitize_value_map(attributes, DEFAULT_FIELD_PREVIEW_CHARS),
        };
        self.record(&record)?;
        Ok(Some(record))
    }

    fn ensure_metadata_root(&mut self) {
        let jsonl_path = if self.config.jsonl {
            Some(self.jsonl_path().to_string_lossy().to_string())
        } else {
            None
        };
        let root = self
            .session_metadata
            .entry(LOGGING_METADATA_KEY.to_string())
            .or_insert_with(|| json!({}));
        if !root.is_object() {
            *root = json!({});
        }
        if let Some(object) = root.as_object_mut() {
            object
                .entry("records".to_string())
                .or_insert_with(|| json!([]));
            object
                .entry("record_count".to_string())
                .or_insert_with(|| json!(0));
            object.insert("level".to_string(), json!(self.config.level));
            object.insert("run_id".to_string(), json!(self.run_id));
            object.insert("trace_id".to_string(), json!(self.trace_id));
            object.insert(
                "jsonl_path".to_string(),
                jsonl_path.map_or(Value::Null, Value::String),
            );
        }
    }

    fn record(&mut self, record: &RuntimeLogRecord) -> SessionResult<()> {
        self.ensure_metadata_root();
        if let Some(Value::Object(root)) = self.session_metadata.get_mut(LOGGING_METADATA_KEY) {
            let count = root
                .get("record_count")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                + 1;
            root.insert("record_count".to_string(), json!(count));
            root.insert("last_log_at_ms".to_string(), json!(record.timestamp_ms));
            if self.config.keep_records {
                let mut records = root
                    .get("records")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                records.push(serde_json::to_value(record)?);
                let max_records = self.config.max_records.max(1) as usize;
                if records.len() > max_records {
                    records = records[records.len() - max_records..].to_vec();
                }
                root.insert("records".to_string(), Value::Array(records));
            }
        }
        if self.config.jsonl {
            append_jsonl(&self.jsonl_path(), record)?;
        }
        Ok(())
    }

    fn jsonl_path(&self) -> PathBuf {
        let root = PathBuf::from(&self.config.jsonl_dir);
        let root = if root.is_absolute() {
            root
        } else {
            self.base_dir.join(root)
        };
        root.join(&self.session_id).join(format!(
            "{}.jsonl",
            self.run_id.as_deref().unwrap_or("run_unbound")
        ))
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RuntimeWarningConfig {
    pub enabled: bool,
    pub context_usage_ratio: Option<f64>,
    pub context_critical_ratio: Option<f64>,
    pub max_step_input_tokens: Option<u64>,
    pub max_step_output_tokens: Option<u64>,
    pub max_step_total_tokens: Option<u64>,
    pub max_step_cost: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeWarningRecord {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub metrics: BTreeMap<String, Value>,
}

impl RuntimeWarningRecord {
    #[must_use]
    pub fn to_event(&self) -> Value {
        json!({
            "type": "runtime-warning",
            "severity": self.severity,
            "code": self.code,
            "message": self.message,
            "metrics": self.metrics,
            "display": {
                "kind": "runtime_warning",
                "severity": self.severity,
                "title": warning_title(&self.code),
                "body": self.message,
                "metrics": display_metrics(&self.code, &self.metrics),
            },
        })
    }
}

#[must_use]
pub fn step_usage_warnings(
    config: &RuntimeWarningConfig,
    usage: &Usage,
    step_index: u64,
) -> Vec<RuntimeWarningRecord> {
    if !config.enabled {
        return Vec::new();
    }
    let total_tokens = usage.input_tokens + usage.output_tokens;
    let base_metrics = BTreeMap::from([
        ("step_index".to_string(), json!(step_index)),
        ("input_tokens".to_string(), json!(usage.input_tokens)),
        ("output_tokens".to_string(), json!(usage.output_tokens)),
        ("total_tokens".to_string(), json!(total_tokens)),
        ("cost".to_string(), json!(usage.cost)),
    ]);
    let mut warnings = Vec::new();
    if config
        .max_step_input_tokens
        .is_some_and(|threshold| usage.input_tokens > threshold)
    {
        let threshold = config.max_step_input_tokens.unwrap_or_default();
        warnings.push(RuntimeWarningRecord {
            code: "step_input_tokens_exceeded".to_string(),
            severity: "warning".to_string(),
            message: format!(
                "Step input tokens exceeded budget: {} > {threshold}.",
                usage.input_tokens
            ),
            metrics: metrics_with_threshold(&base_metrics, threshold),
        });
    }
    if config
        .max_step_output_tokens
        .is_some_and(|threshold| usage.output_tokens > threshold)
    {
        let threshold = config.max_step_output_tokens.unwrap_or_default();
        warnings.push(RuntimeWarningRecord {
            code: "step_output_tokens_exceeded".to_string(),
            severity: "warning".to_string(),
            message: format!(
                "Step output tokens exceeded budget: {} > {threshold}.",
                usage.output_tokens
            ),
            metrics: metrics_with_threshold(&base_metrics, threshold),
        });
    }
    if config
        .max_step_total_tokens
        .is_some_and(|threshold| total_tokens > threshold)
    {
        let threshold = config.max_step_total_tokens.unwrap_or_default();
        warnings.push(RuntimeWarningRecord {
            code: "step_total_tokens_exceeded".to_string(),
            severity: "warning".to_string(),
            message: format!("Step total tokens exceeded budget: {total_tokens} > {threshold}."),
            metrics: metrics_with_threshold(&base_metrics, threshold),
        });
    }
    if config
        .max_step_cost
        .is_some_and(|threshold| usage.cost > threshold)
    {
        let threshold = config.max_step_cost.unwrap_or_default();
        let mut metrics = base_metrics.clone();
        metrics.insert("threshold".to_string(), json!(threshold));
        warnings.push(RuntimeWarningRecord {
            code: "step_cost_exceeded".to_string(),
            severity: "warning".to_string(),
            message: format!(
                "Step cost exceeded budget: {:.6} > {threshold:.6}.",
                usage.cost
            ),
            metrics,
        });
    }
    warnings
}

#[must_use]
pub fn format_runtime_warning_event(event: &Value) -> Option<String> {
    if event.get("type").and_then(Value::as_str) != Some("runtime-warning") {
        return None;
    }
    let display = event.get("display").and_then(Value::as_object);
    let severity = display
        .and_then(|items| items.get("severity"))
        .or_else(|| event.get("severity"))
        .and_then(Value::as_str)
        .unwrap_or("warning")
        .to_uppercase();
    let title = display
        .and_then(|items| items.get("title"))
        .or_else(|| event.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("Runtime warning");
    let body = display
        .and_then(|items| items.get("body"))
        .or_else(|| event.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let metric_text = display
        .and_then(|items| items.get("metrics"))
        .and_then(Value::as_object)
        .map(format_display_metrics)
        .unwrap_or_default();
    let suffix = if metric_text.is_empty() {
        String::new()
    } else {
        format!(" ({metric_text})")
    };
    Some(format!("[{severity}] {title}: {body}{suffix}"))
}

#[must_use]
pub fn sanitize_trace_value(value: Value) -> Value {
    sanitize_value(value, DEFAULT_FIELD_PREVIEW_CHARS)
}

#[must_use]
pub fn sanitize_observation_value(value: Value) -> Value {
    sanitize_value(value, DEFAULT_FIELD_PREVIEW_CHARS)
}

#[must_use]
pub fn input_preview(value: Value, max_chars: usize) -> String {
    truncate_text(
        &python_json_dumps(&sanitize_value(value, max_chars)),
        max_chars,
    )
}

#[must_use]
pub fn output_stats(output: &str) -> BTreeMap<String, u64> {
    BTreeMap::from([
        ("output_bytes".to_string(), output.len() as u64),
        ("output_lines".to_string(), output.lines().count() as u64),
    ])
}

fn stored_message(message: &ChatMessage, index: u64) -> StoredMessage {
    StoredMessage {
        message_id: message_id(message),
        index,
        role: message.role.clone(),
        content: message.content.clone(),
        name: message.name.clone(),
        tool_call_id: message.tool_call_id.clone(),
        metadata: message.metadata.clone(),
    }
}

fn message_id(message: &ChatMessage) -> String {
    message
        .metadata
        .get("message_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| new_id("msg"))
}

fn session_status_str(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::Paused => "paused",
        SessionStatus::Stop => "stop",
        SessionStatus::Compacting => "compacting",
    }
}

fn session_status_from_value(value: Option<&Value>) -> SessionStatus {
    match value.and_then(Value::as_str) {
        Some("running") => SessionStatus::Running,
        Some("paused") => SessionStatus::Paused,
        Some("stop") => SessionStatus::Stop,
        Some("compacting") => SessionStatus::Compacting,
        _ => SessionStatus::Idle,
    }
}

fn count_events(events: &[Value], name: &str) -> u64 {
    events
        .iter()
        .filter(|event| event.get("event").and_then(Value::as_str) == Some(name))
        .count() as u64
}

fn count_by_key(rows: &[Value], key: &str) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for row in rows {
        if let Some(value) = row.get(key).and_then(Value::as_str) {
            *counts.entry(value.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

fn tool_source(attrs: &Map<String, Value>) -> &'static str {
    if let Some(source) = attrs
        .get("tool_source")
        .or_else(|| attrs.get("source"))
        .and_then(Value::as_str)
    {
        if !source.is_empty() {
            return match source {
                "mcp" => "mcp",
                "skill" => "skill",
                "local" => "local",
                "local_tool" => "local_tool",
                _ => "unknown",
            };
        }
    }
    if attrs.get("backend").and_then(Value::as_str) == Some("mcp") {
        return "mcp";
    }
    if attrs.get("skill_name").is_some()
        || attrs.get("tool_group").and_then(Value::as_str) == Some("skill")
    {
        return "skill";
    }
    if attrs.get("tool_group").and_then(Value::as_str).is_some() {
        return "local_tool";
    }
    "unknown"
}

fn inc_summary_u64(summary: &mut Map<String, Value>, key: &str, amount: u64) {
    let current = summary.get(key).and_then(Value::as_u64).unwrap_or_default();
    summary.insert(key.to_string(), json!(current + amount));
}

fn inc_summary_f64(summary: &mut Map<String, Value>, key: &str, amount: f64) {
    let current = summary.get(key).and_then(Value::as_f64).unwrap_or_default();
    summary.insert(key.to_string(), json!(current + amount));
}

fn u64_field(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn write_json(path: &Path, payload: &impl Serialize) -> SessionResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|item| item.to_str())
            .unwrap_or("json")
    ));
    fs::write(&tmp, serde_json::to_string_pretty(payload)? + "\n")?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn append_jsonl(path: &Path, payload: &impl Serialize) -> SessionResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(serde_json::to_string(payload)?.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

fn read_json_object(path: &Path) -> SessionResult<Option<Map<String, Value>>> {
    if !path.exists() {
        return Ok(None);
    }
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
    Ok(value.as_object().cloned())
}

fn read_jsonl(path: &Path) -> SessionResult<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).map_err(Into::into))
        .collect()
}

fn next_seq(path: &Path) -> SessionResult<u64> {
    Ok(read_jsonl(path)?.len() as u64 + 1)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", now_ms())
}

fn bool_option(value: &Value, default: bool) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::String(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "on" | "true" | "yes")
                || (!matches!(normalized.as_str(), "0" | "false" | "no" | "off") && default)
        }
        Value::Null => default,
        _ => value.as_bool().unwrap_or(default),
    }
}

fn sanitize_value_map(map: BTreeMap<String, Value>, max_chars: usize) -> BTreeMap<String, Value> {
    map.into_iter()
        .map(|(key, value)| {
            if is_sensitive_key(&key) && !SAFE_TOKEN_METRIC_KEYS.contains(&key.as_str()) {
                (key, json!("[redacted]"))
            } else {
                (key, sanitize_value(value, max_chars))
            }
        })
        .collect()
}

fn sanitize_value(value: Value, max_chars: usize) -> Value {
    match value {
        Value::Object(items) => Value::Object(
            items
                .into_iter()
                .map(|(key, value)| {
                    if is_sensitive_key(&key) && !SAFE_TOKEN_METRIC_KEYS.contains(&key.as_str()) {
                        (key, json!("[redacted]"))
                    } else {
                        (key, sanitize_value(value, max_chars))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| sanitize_value(item, max_chars))
                .collect(),
        ),
        Value::String(value) => Value::String(truncate_text(&value, max_chars)),
        other => other,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SENSITIVE_KEY_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let len = value.chars().count();
    if max_chars == 0 {
        return String::new();
    }
    if len <= max_chars {
        return value.to_string();
    }
    let hidden = len.saturating_sub(max_chars.saturating_sub(24));
    let suffix = format!("...[truncated {hidden} chars]");
    let prefix_len = max_chars.saturating_sub(suffix.chars().count());
    format!(
        "{}{}",
        value.chars().take(prefix_len).collect::<String>(),
        suffix
    )
}

fn python_json_dumps(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => {
            if *value {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(python_json_dumps)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(items) => format!(
            "{{{}}}",
            items
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    serde_json::to_string(key).unwrap_or_default(),
                    python_json_dumps(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn normalize_level(level: &str) -> String {
    match level.to_ascii_uppercase().as_str() {
        "DEBUG" | "INFO" | "WARNING" | "ERROR" | "CRITICAL" => level.to_ascii_uppercase(),
        _ => "INFO".to_string(),
    }
}

fn level_number(level: &str) -> u8 {
    match normalize_level(level).as_str() {
        "DEBUG" => 10,
        "INFO" => 20,
        "WARNING" => 30,
        "ERROR" => 40,
        "CRITICAL" => 50,
        _ => 20,
    }
}

fn metrics_with_threshold(
    base: &BTreeMap<String, Value>,
    threshold: u64,
) -> BTreeMap<String, Value> {
    let mut metrics = base.clone();
    metrics.insert("threshold".to_string(), json!(threshold));
    metrics
}

fn warning_title(code: &str) -> String {
    match code {
        "context_usage_high" => "Context usage high".to_string(),
        "context_usage_critical" => "Context usage critical".to_string(),
        "step_input_tokens_exceeded" => "Step input token budget exceeded".to_string(),
        "step_output_tokens_exceeded" => "Step output token budget exceeded".to_string(),
        "step_total_tokens_exceeded" => "Step token budget exceeded".to_string(),
        "step_cost_exceeded" => "Step cost budget exceeded".to_string(),
        other => title_case(&other.replace('_', " ")),
    }
}

fn title_case(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_metrics(code: &str, metrics: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    let keys = if code.starts_with("context_usage_") {
        vec![
            "step_index",
            "usage_ratio",
            "threshold",
            "estimated_input_tokens",
            "input_limit_tokens",
            "fallback_stage",
        ]
    } else if code == "step_cost_exceeded" {
        vec![
            "step_index",
            "cost",
            "threshold",
            "input_tokens",
            "output_tokens",
            "total_tokens",
        ]
    } else if code.starts_with("step_") {
        vec![
            "step_index",
            "input_tokens",
            "output_tokens",
            "total_tokens",
            "threshold",
        ]
    } else {
        return metrics.clone();
    };
    keys.into_iter()
        .filter_map(|key| {
            metrics
                .get(key)
                .filter(|value| !value.is_null())
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect()
}

fn format_display_metrics(metrics: &Map<String, Value>) -> String {
    metrics
        .iter()
        .map(|(key, value)| {
            let text = if let Some(number) = value.as_f64() {
                if (key.ends_with("ratio") || key == "threshold") && number > 0.0 && number <= 1.0 {
                    format!("{:.1}%", number * 100.0)
                } else {
                    format_float(number)
                }
            } else {
                value
                    .as_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| value.to_string())
            };
            format!("{key}={text}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_float(value: f64) -> String {
    let rendered = format!("{value:.6}");
    rendered
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_protocol_crate() {
        assert_eq!(crate_name(), "openagent-session");
        assert_eq!(protocol_crate_name(), "openagent-protocol");
    }
}
