//! Workspace runtime, tool registry, and built-in tools for the Rust rewrite.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use openagent_core::{
    PermissionManager, SkillDiscoveryReport, SkillRegistry, pattern_for, render_skill_document,
};
use openagent_protocol::{
    PermissionAction, PermissionRuleset, ToolConcurrency, ToolExecutionSchema, ToolExecutionScope,
    ToolResult, ToolSchema,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const TASK_TOOL_ID: &str = "task";

const DEFAULT_READ_LIMIT: usize = 2000;
const MAX_LINE_LENGTH: usize = 2000;
const MAX_READ_BYTES: usize = 50 * 1024;
const DEFAULT_TOOL_MAX_LINES: usize = 2000;
const DEFAULT_TOOL_MAX_BYTES: usize = 50 * 1024;
const GLOB_LIMIT: usize = 100;
const GREP_LIMIT: usize = 100;
const LS_LIMIT: usize = 100;
const CODE_SEARCH_MAX_HITS: usize = 200;
const CODE_SEARCH_MAX_PREVIEW_HITS: usize = 20;
const CODE_SEARCH_MAX_LINE_CHARS: usize = 240;

const DEFAULT_LS_IGNORE: &[&str] = &[
    "node_modules/",
    "__pycache__/",
    ".git/",
    "dist/",
    "build/",
    "target/",
    "vendor/",
    ".idea/",
    ".vscode/",
    ".venv/",
    "venv/",
    "env/",
    "coverage/",
];

const BINARY_EXTENSIONS: &[&str] = &[
    ".zip", ".tar", ".gz", ".exe", ".dll", ".so", ".class", ".jar", ".war", ".7z", ".doc", ".docx",
    ".xls", ".xlsx", ".ppt", ".pptx", ".odt", ".ods", ".odp", ".bin", ".dat", ".obj", ".o", ".a",
    ".lib", ".wasm", ".pyc", ".pyo", ".pdf", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico",
];

type ToolResultValue<T> = Result<T, String>;

include!("toolkit/types.rs");
include!("toolkit/toolkit.rs");
include!("toolkit/builtin_registry.rs");
include!("toolkit/path_security.rs");
include!("toolkit/builtin_tools.rs");
include!("toolkit/schema_helpers.rs");
include!("toolkit/path_helpers.rs");
include!("toolkit/grep.rs");
include!("toolkit/ls.rs");
include!("toolkit/output_helpers.rs");
include!("toolkit/tests.rs");
