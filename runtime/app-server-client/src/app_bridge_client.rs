//! App Bridge client-side state for the Rust rewrite.

use std::{collections::BTreeSet, path::Path, time::Duration};

use openagent_app_server::AppEvent;
use serde_json::{Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

const TERMINAL_METHODS: &[&str] = &["turn/completed", "turn/failed", "turn/interrupted"];
const TERMINAL_STATUSES: &[&str] = &["completed", "failed", "interrupted"];

include!("app_bridge_client/types.rs");
include!("app_bridge_client/url.rs");
include!("app_bridge_client/client_core.rs");
include!("app_bridge_client/client_sessions.rs");
include!("app_bridge_client/client_turns.rs");
include!("app_bridge_client/client_transport.rs");
include!("app_bridge_client/sse.rs");
include!("app_bridge_client/events.rs");
include!("app_bridge_client/fixtures.rs");
include!("app_bridge_client/helpers.rs");
include!("app_bridge_client/tests.rs");
