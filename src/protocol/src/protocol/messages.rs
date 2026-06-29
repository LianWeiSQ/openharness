#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub name: Option<String>,
    pub tool_call_id: Option<String>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Pending,
    Running,
    #[default]
    Completed,
    Error,
    Interrupted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessagePartKind {
    Text,
    Tool,
    Reasoning,
    File,
    Approval,
    Question,
    Usage,
    Patch,
    Compaction,
    Context,
    Subtask,
}

impl MessagePartKind {
    #[must_use]
    pub fn from_type(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "tool" | "tool_call" | "tool_result" => Self::Tool,
            "reasoning" => Self::Reasoning,
            "file" | "attachment" => Self::File,
            "approval" | "permission" => Self::Approval,
            "question" => Self::Question,
            "usage" => Self::Usage,
            "patch" | "diff" => Self::Patch,
            "compaction" | "compact" => Self::Compaction,
            "context" | "context_asset" => Self::Context,
            "subtask" => Self::Subtask,
            _ => Self::Text,
        }
    }

    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Tool => "tool",
            Self::Reasoning => "reasoning",
            Self::File => "file",
            Self::Approval => "approval",
            Self::Question => "question",
            Self::Usage => "usage",
            Self::Patch => "patch",
            Self::Compaction => "compaction",
            Self::Context => "context",
            Self::Subtask => "subtask",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MessageInfo {
    pub id: String,
    pub session_id: String,
    pub role: Role,
    pub created_at_ms: u64,
    pub run_id: Option<String>,
    pub step_index: Option<u64>,
    pub status: MessageStatus,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MessagePart {
    pub id: String,
    pub message_id: String,
    pub session_id: String,
    pub seq: u64,
    pub kind: MessagePartKind,
    pub status: MessageStatus,
    pub content: Value,
    pub attributes: BTreeMap<String, Value>,
    pub timestamp_ms: u64,
    pub run_id: Option<String>,
    pub step_index: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MessageWithParts {
    pub info: MessageInfo,
    pub parts: Vec<MessagePart>,
}
