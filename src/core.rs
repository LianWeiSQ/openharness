//! Core permission, context, instruction, and skill behavior for the Rust rewrite.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use openagent_protocol::{
    ChatMessage, MaterializedPayload, Model, PermissionAction, PermissionRule, PermissionRuleset,
    Role, ToolSchema, Usage, WorkState, WorkStateFile, materialize_openai_compatible_payload,
    render_work_state, ruleset,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

pub const DEFAULT_BYTES_PER_TOKEN: u64 = 3;
pub const DEFAULT_GUARD_RATIO: f64 = 0.9;
pub const DEFAULT_INPUT_SAFETY_MARGIN_TOKENS: u64 = 1024;
pub const DEFAULT_TOOL_DISPLAY_MAX_BYTES: u64 = 50 * 1024;
pub const DEFAULT_TOOL_CONTEXT_PREVIEW_BYTES: u64 = 4096;
pub const DEFAULT_TOOL_CONTEXT_PREVIEW_LINES: u64 = 40;
pub const DEFAULT_TOOL_CONTEXT_LINE_MAX_CHARS: u64 = 240;
pub const DEFAULT_PRUNE_OLD_TOOL_OUTPUTS: bool = true;
pub const DEFAULT_PRUNE_KEEP_RECENT_USER_TURNS: u64 = 2;
pub const DEFAULT_PRUNE_PROTECT_INPUT_TOKENS: u64 = 12_000;
pub const DEFAULT_PRUNE_MIN_INPUT_TOKENS: u64 = 4_000;
pub const DEFAULT_COMPACT_SUMMARY_MAX_OUTPUT_TOKENS: u64 = 512;
pub const DEFAULT_COMPACT_REFRESH_MIN_NEW_MESSAGES: u64 = 6;
pub const DEFAULT_OVERFLOW_KEEP_RECENT_USER_TURNS: u64 = 2;
pub const DEFAULT_COMPACTION_MODE: &str = "structured_work_state";

pub const DEFAULT_MAX_FILE_BYTES: usize = 16 * 1024;
pub const DEFAULT_MAX_TOTAL_BYTES: usize = 48 * 1024;
pub const DEFAULT_WORKSPACE_FILES: &[&str] = &["OPENAGENT.md", "AGENTS.md", "CLAUDE.md"];
pub const DEFAULT_USER_FILES: &[&str] = &["OPENAGENT.md", "instructions.md"];

const SUPPORTED_STRATEGIES: &[&str] = &["auto", "error", "compact"];
const SUPPORTED_COUNTING: &[&str] = &["auto", "tiktoken", "heuristic"];
const SUPPORTED_COMPACTION_MODES: &[&str] = &["structured_work_state"];

include!("core/permissions.rs");
include!("core/context_budget.rs");
include!("core/context_pack.rs");
include!("core/instructions.rs");
include!("core/skills.rs");
include!("core/scripted_loop.rs");
include!("core/context_helpers.rs");
include!("core/instruction_helpers.rs");
include!("core/skill_helpers.rs");
include!("core/util.rs");
include!("core/tests.rs");
