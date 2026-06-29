//! Model provider adapters for the Rust rewrite.

use std::collections::{BTreeMap, BTreeSet};

use openagent_protocol::{
    ChatMessage, MaterializedPayload, Model, ModelCapabilities, ModelPricing, RUNTIME_OPTION_KEYS,
    Role, ToolSchema, Usage, materialize_openai_compatible_payload,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const DEFAULT_PROVIDER: &str = "openai";
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-5";
pub const DEFAULT_ANTHROPIC_CONTEXT_WINDOW: u64 = 200_000;
pub const DEFAULT_ANTHROPIC_MAX_OUTPUT: u64 = 8192;
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
pub const HTTP_ERROR_BODY_PREVIEW_CHARS: usize = 800;

include!("provider/types.rs");
include!("provider/openai_chat.rs");
include!("provider/openai_responses.rs");
include!("provider/anthropic.rs");
include!("provider/stream_state.rs");
include!("provider/provider_metadata.rs");
include!("provider/parsing_helpers.rs");
include!("provider/responses_helpers.rs");
include!("provider/anthropic_helpers.rs");
include!("provider/tests.rs");
