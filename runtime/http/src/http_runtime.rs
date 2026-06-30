//! HTTP runtime service contracts for the Rust rewrite.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use openagent_app_server::{
    approval_response_payload, control_next_payload, parse_turn_approval_path,
    parse_turn_question_reply_path, question_dismiss_payload, question_reply_payload,
    record_control_response_payload, tui_control_request_for_path,
};
use openagent_core::{PermissionManager, permission_rule};
use openagent_protocol::{
    ChatMessage, PermissionAction, PermissionRuleset, Role, ToolCall, ToolResult, ToolSchema, Usage,
};
use openagent_provider::{
    OpenAiLanguageModelConfig, ProviderStreamEvent, build_openai_chat_payload,
    build_openai_responses_payload, default_env_mapping, normalize_openai_chat_sse_chunks,
    normalize_openai_responses_response, normalize_openai_responses_stream_events,
    normalize_provider, parse_tool_arguments, provider_default_base_url, provider_default_model,
    provider_requires_api_key, summarize_http_error_body,
};
use openagent_session::{
    FileSessionStore, Session, SessionEventOptions, SessionPartOptions, SessionStatus,
    StartRunOptions,
};
use openagent_tools::{
    TASK_TOOL_ID, TaskPermissionRule, TaskSubagentDescriptor, ToolContext, Toolkit,
    register_task_tool, resolve_path_in_root, task_subagent_is_visible,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 8787;
const INDEX_HTML: &str = include_str!("../../static/app-server/static/index.html");
const APP_JS: &str = include_str!("../../static/app-server/static/app.js");
const APP_CSS: &str = include_str!("../../static/app-server/static/app.css");
const APP_EVENTS_FILE: &str = "app_events.jsonl";
const TUI_CONTROL_QUEUE_FILE: &str = "tui_control_queue.json";
const TUI_CONTROL_RESPONSES_FILE: &str = "tui_control_responses.jsonl";
const FILE_CHANGE_UNDO_STACK_KEY: &str = "file_change_undo_stack";
const FILE_CHANGE_REDO_STACK_KEY: &str = "file_change_redo_stack";
const FILE_CHANGE_LATEST_KEY: &str = "latest_file_change";
const MAX_FILE_CHANGE_STACK: usize = 50;
const MAX_RENDERED_DIFF_LINES: usize = 400;
const DEFAULT_TASK_RUN_LOCK_STALE_MS: u64 = 15 * 60 * 1000;
const DEFAULT_BACKGROUND_TASK_WORKER_POLL_MS: u64 = 100;
const DEFAULT_MAX_SUBAGENT_DEPTH: u64 = 3;
const BUILD_AGENT_PROMPT: &str = include_str!("../../../skill/prompts/build.txt");
const EXPLORE_AGENT_PROMPT: &str = include_str!("../../../skill/prompts/explore.txt");
const PLAN_AGENT_PROMPT: &str = include_str!("../../../skill/prompts/plan.txt");
const REVIEW_AGENT_PROMPT: &str = "You are OpenAgent Reviewer. Focus on correctness, regressions, risk, and missing tests. Prefer evidence from tools and keep findings concise.";

include!("http/types.rs");
include!("http/responses.rs");
include!("http/app_events.rs");
include!("http/prompt.rs");
include!("http/cli.rs");
include!("http/background_tasks.rs");
include!("http/router.rs");
include!("http/session_routes.rs");
include!("http/agent_profiles.rs");
include!("http/session_crud.rs");
include!("http/session_tasks.rs");
include!("http/session_actions.rs");
include!("http/patch_stack.rs");
include!("http/session_summary.rs");
include!("http/provider_profile.rs");
include!("http/provider_loop.rs");
include!("http/turn_routes.rs");
include!("http/event_store.rs");
include!("http/fixtures.rs");
include!("http/tests.rs");
