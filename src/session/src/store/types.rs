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
pub struct StoredMessageV2 {
    pub schema_version: String,
    pub index: u64,
    pub info: MessageInfo,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StoredMessagePartV2 {
    pub schema_version: String,
    pub part: MessagePart,
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
