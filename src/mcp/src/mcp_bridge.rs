//! MCP config, auth, discovery, and tool bridge crate for the Rust rewrite.

use std::{collections::BTreeMap, fs, path::Path};

use openagent_protocol::{ToolConcurrency, ToolExecutionSchema, ToolExecutionScope};
use openagent_tools::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const DEFAULT_REFRESH_TTL_S: f64 = 30.0;
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MIN_TIMEOUT_MS: u64 = 1_000;

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

include!("mcp/types.rs");
include!("mcp/config.rs");
include!("mcp/descriptors.rs");
include!("mcp/transport.rs");
include!("mcp/result.rs");
include!("mcp/sanitize.rs");
include!("mcp/helpers.rs");
