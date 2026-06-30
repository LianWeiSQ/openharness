//! App Bridge protocol and runtime state for the Rust rewrite.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const MAX_TUI_CONTROL_QUEUE: usize = 100;
pub const UNAUTHORIZED_WWW_AUTHENTICATE: &str = "Bearer realm=\"openagent-app-bridge\"";

include!("app_bridge/types.rs");
include!("app_bridge/events.rs");
include!("app_bridge/auth.rs");
include!("app_bridge/interactions.rs");
include!("app_bridge/control.rs");
include!("app_bridge/fixtures.rs");
include!("app_bridge/json_helpers.rs");
include!("app_bridge/tests.rs");
