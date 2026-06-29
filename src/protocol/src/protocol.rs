//! Shared protocol contracts for OpenAgent.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

pub const RUNTIME_OPTION_KEYS: &[&str] = &[
    "compaction",
    "context_budget",
    "logging",
    "observability",
    "runtime_warnings",
    "session_store",
    "trace",
];

include!("protocol/model.rs");
include!("protocol/messages.rs");
include!("protocol/tools.rs");
include!("protocol/usage_stream.rs");
include!("protocol/permissions.rs");
include!("protocol/openai_materialization.rs");
include!("protocol/part_projection.rs");
include!("protocol/swarm.rs");
include!("protocol/tool_execution.rs");
include!("protocol/work_state.rs");
include!("protocol/helpers.rs");
include!("protocol/tests.rs");
