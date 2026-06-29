//! Session, trace, and observability crate for the Rust rewrite.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use openagent_protocol::{
    ChatMessage, MessageInfo, MessagePart, MessagePartKind, MessageStatus, MessageWithParts, Role,
    Usage, message_parts_to_chat_messages,
};
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

include!("store/types.rs");
include!("store/file_store.rs");
include!("store/event_options.rs");
include!("store/trace.rs");
include!("store/observation.rs");
include!("store/logging.rs");
include!("store/warnings.rs");
include!("store/message_parts.rs");
include!("store/summary_helpers.rs");
include!("store/io.rs");
include!("store/tests.rs");
