//! Eval, replay, CI gate, and benchmark contracts for the Rust rewrite.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const FIXTURE_ROOT: &str = "/tmp/openagent-rust-rewrite-fixture-goal13";

pub const DEFAULT_MAX_STEPS: i64 = 80;
pub const DEFAULT_CONTEXT_WINDOW: i64 = 128_000;
pub const DEFAULT_MAX_OUTPUT: i64 = 4096;
pub const DEFAULT_WORKDIR: &str = "/app";

include!("eval/types.rs");
include!("eval/summary.rs");
include!("eval/regression.rs");
include!("eval/ci_gate.rs");
include!("eval/langfuse.rs");
include!("eval/terminal.rs");
include!("eval/harbor.rs");
include!("eval/fixtures.rs");
include!("eval/fixture_results.rs");
include!("eval/helpers.rs");
include!("eval/tests.rs");
