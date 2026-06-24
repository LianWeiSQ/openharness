//! CLI crate for the Rust rewrite.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use openagent_mcp::{
    McpTransport, RemoteMcpManager, RemoteMcpServerConfig, RemoteMcpToolDescriptor,
    bridge_tool_output, build_tool_descriptors_from_values, load_mcp_config, mcp_tool_definition,
    normalize_tool_call_result, sanitize_mcp_observation_value, transport_candidates,
    unavailable_tool_result,
};
use openagent_protocol::ToolResult;
use openagent_protocol::{ChatMessage, PermissionRuleset, Role, ToolCall, ToolSchema, Usage};
use openagent_provider::{
    AnthropicLanguageModelConfig, OpenAiLanguageModelConfig, ProviderStreamEvent, anthropic_model,
    build_anthropic_payload, build_openai_chat_payload, build_openai_responses_payload,
    default_env_mapping, normalize_anthropic_events, normalize_openai_chat_sse_chunks,
    normalize_openai_responses_response, normalize_provider, openai_compatible_model,
    parse_tool_arguments, provider_auth_methods, provider_default_base_url, provider_default_model,
    provider_label, provider_requires_api_key, summarize_http_error_body,
};
use openagent_session::{
    FileSessionStore, Session, SessionEventOptions, SessionPartOptions, SessionStatus,
    StartRunOptions,
};
use openagent_tools::{ToolContext, Toolkit};
use serde_json::{Map, Value, json};

mod agents;
mod mcp;
mod models;
mod prompt;
mod remote;

use agents::{agent_command, agent_registry_dir, plugin_command};
use mcp::mcp_command;
use models::{models_cache_path, models_command};
use prompt::{run_prompt_command, run_prompt_command_with_events, split_answer_items};
use remote::{
    attach_command, http_runtime_command, remote_auth_from_args, remote_events_for_payload,
    remote_select_session, remote_select_session_with_auth, remote_start_turn,
    remote_start_turn_with_auth, text_from_app_events, tui_command,
};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const DEFAULT_BASE_URL: &str = "http://localhost:8080";
pub const DEFAULT_MODEL: &str = "gpt-5.5";
pub const DEFAULT_WIRE_API: &str = "responses";
pub const DEFAULT_MAX_STEPS: &str = "30";
pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8787";
pub const DEFAULT_SERVER_TOKEN_ENV: &str = "OPENAGENT_SERVER_TOKEN";
pub const GOAL10_ROOT: &str = "/tmp/openagent-rust-rewrite-fixture-goal10";
pub const GOAL10_WORKSPACE: &str = "/tmp/openagent-rust-rewrite-fixture-goal10/workspace";
const RUN_POSITIONAL_VALUE_FLAGS: &[&str] = &[
    "--workspace",
    "--dir",
    "--config",
    "--auth-file",
    "--base-url",
    "--session-root",
    "--session",
    "-s",
    "--file",
    "-f",
    "--command",
    "--command-dir",
    "--format",
    "--model",
    "-m",
    "--provider",
    "--wire-api",
    "--api-key",
    "--mcp-config",
    "--max-steps",
    "--max-output-tokens",
    "--timeout-s",
    "--agent",
    "--title",
    "--attach",
    "--server-token",
    "--server-token-env",
    "--answer",
    "--permission",
    "--password",
    "-p",
    "--username",
    "-u",
    "--variant",
    "--port",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliRunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CliRunResult {
    #[must_use]
    pub fn ok_json(value: &Value) -> Self {
        Self {
            exit_code: 0,
            stdout: format!("{}\n", python_json_dumps(value)),
            stderr: String::new(),
        }
    }
}

fn ok_text(text: impl Into<String>) -> CliRunResult {
    CliRunResult {
        exit_code: 0,
        stdout: ensure_trailing_newline(text.into()),
        stderr: String::new(),
    }
}

fn err_text(exit_code: i32, text: impl Into<String>) -> CliRunResult {
    CliRunResult {
        exit_code,
        stdout: String::new(),
        stderr: ensure_trailing_newline(text.into()),
    }
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn is_help_flag(arg: &str) -> bool {
    matches!(arg, "--help" | "-h")
}

struct HelpSection<'a> {
    title: &'a str,
    rows: &'a [(&'a str, &'a str)],
}

fn render_help_screen(
    title: &str,
    summary: &str,
    usage: &str,
    sections: &[HelpSection<'_>],
    examples: &[&str],
) -> String {
    let mut output = String::new();
    output.push_str(title);
    output.push('\n');
    if !summary.is_empty() {
        output.push_str("  ");
        output.push_str(summary);
        output.push('\n');
    }
    output.push('\n');
    output.push_str("Usage\n");
    output.push_str("  ");
    output.push_str(usage);
    output.push('\n');
    for section in sections {
        output.push('\n');
        output.push_str(section.title);
        output.push('\n');
        push_aligned_rows(&mut output, section.rows);
    }
    if !examples.is_empty() {
        output.push('\n');
        output.push_str("Examples\n");
        for example in examples {
            output.push_str("  ");
            output.push_str(example);
            output.push('\n');
        }
    }
    output.trim_end().to_string()
}

fn push_aligned_rows(output: &mut String, rows: &[(&str, &str)]) {
    let width = rows
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0)
        .clamp(12, 34);
    for (label, description) in rows {
        output.push_str("  ");
        if label.len() > width {
            output.push_str(label);
            output.push('\n');
            output.push_str("  ");
            output.push_str(&" ".repeat(width));
            output.push_str("  ");
            output.push_str(description);
            output.push('\n');
        } else {
            output.push_str(&format!("{label:<width$}  {description}"));
            output.push('\n');
        }
    }
}

fn render_key_values(title: &str, rows: &[(&str, String)]) -> String {
    let mut output = String::new();
    output.push_str(title);
    output.push('\n');
    let width = rows
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0)
        .clamp(8, 24);
    for (label, value) in rows {
        output.push_str("  ");
        output.push_str(&format!("{label:<width$}  {value}"));
        output.push('\n');
    }
    output.trim_end().to_string()
}

fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut widths = headers
        .iter()
        .map(|header| header.len())
        .collect::<Vec<_>>();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(cell.len());
            }
        }
    }
    let mut output = String::new();
    push_table_row(&mut output, headers, &widths);
    let separators = widths
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>();
    let separator_refs = separators.iter().map(String::as_str).collect::<Vec<_>>();
    push_table_row(&mut output, &separator_refs, &widths);
    for row in rows {
        let row_refs = row.iter().map(String::as_str).collect::<Vec<_>>();
        push_table_row(&mut output, &row_refs, &widths);
    }
    output.trim_end().to_string()
}

fn push_table_row(output: &mut String, cells: &[&str], widths: &[usize]) {
    output.push_str("  ");
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            output.push_str("  ");
        }
        let width = widths.get(index).copied().unwrap_or(cell.len());
        output.push_str(&format!("{cell:<width$}"));
    }
    while output.ends_with(' ') {
        output.pop();
    }
    output.push('\n');
}

fn compact_text_value(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::String(value) if value.is_empty() => "-".to_string(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => {
            if *value {
                "yes".to_string()
            } else {
                "no".to_string()
            }
        }
        Value::Number(value) => value.to_string(),
        _ => python_json_dumps(value),
    }
}

fn has_flag(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|arg| names.contains(&arg.as_str()))
}

fn value_for(args: &[String], names: &[&str]) -> Option<String> {
    for arg in args {
        if let Some((name, value)) = arg.split_once('=')
            && names.contains(&name)
        {
            return Some(value.to_string());
        }
    }
    args.windows(2)
        .find(|items| names.contains(&items[0].as_str()))
        .and_then(|items| items.get(1))
        .cloned()
}

fn values_for(args: &[String], names: &[&str]) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if names.contains(&args[index].as_str()) && index + 1 < args.len() {
            if let Some(value) = args.get(index + 1) {
                values.push(value.clone());
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    values
}

fn positional_args(args: &[String], value_flags: &[&str]) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            values.extend(args.iter().skip(index + 1).cloned());
            break;
        }
        if arg.starts_with('-') {
            index += if value_flags.contains(&arg.as_str()) && index + 1 < args.len() {
                2
            } else {
                1
            };
            continue;
        }
        values.push(arg.clone());
        index += 1;
    }
    values
}

#[allow(dead_code)]
fn simple_command(
    name: &str,
    args: &[String],
    help: &'static str,
    non_help_message: &'static str,
) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(help);
    }
    CliRunResult {
        exit_code: 0,
        stdout: format!("{non_help_message}\n"),
        stderr: if name == "tui" {
            String::new()
        } else {
            "Use --help for flags; live long-running server/UI execution is validated by the dedicated Rust binary crates.\n".to_string()
        },
    }
}

fn root_help() -> String {
    let core = [
        ("run", "run an agent prompt without opening the TUI"),
        ("tui", "start the terminal UI"),
        ("attach", "attach to a running App Bridge server"),
        ("serve", "start the local App Bridge HTTP server"),
        ("web", "start the browser console server"),
        ("client", "send a prompt to a running App Bridge server"),
    ];
    let workspace = [
        (
            "session",
            "list, export, import, share, and delete sessions",
        ),
        ("models", "inspect provider and model metadata"),
        ("stats", "show local session usage statistics"),
        ("command", "manage custom prompt commands"),
        ("config", "inspect and initialize CLI configuration"),
        ("doctor", "check provider and gateway configuration"),
    ];
    let integrations = [
        ("auth", "manage provider credentials"),
        ("providers", "provider credential alias for auth"),
        ("mcp", "manage remote MCP servers"),
        ("approval", "inspect or answer queued approval requests"),
        ("question", "inspect or answer queued question requests"),
    ];
    let parity = [
        ("agent, plugin", "agent profiles and plugin command surface"),
        ("github, pr", "GitHub issue, workflow, and PR helpers"),
        (
            "debug, db, acp",
            "diagnostics, local database, and ACP helpers",
        ),
        (
            "upgrade, uninstall, import, export, generate, console",
            "OpenCode parity lifecycle and utility commands",
        ),
    ];
    render_help_screen(
        "OpenAgent CLI",
        "Agent workflows, sessions, tools, and model routing from one terminal.",
        "openagent <command> [options]",
        &[
            HelpSection {
                title: "Core Commands",
                rows: &core,
            },
            HelpSection {
                title: "Workspace",
                rows: &workspace,
            },
            HelpSection {
                title: "Integrations",
                rows: &integrations,
            },
            HelpSection {
                title: "OpenCode Parity Surface",
                rows: &parity,
            },
        ],
        &[
            "openagent run --stream \"summarize this repo\"",
            "openagent tui --attach http://127.0.0.1:8787",
            "openagent models --catalog --verbose",
        ],
    )
}

fn run_help() -> String {
    let session = [
        ("-c, --continue", "continue the latest session"),
        ("-s, --session <id>", "continue a specific session"),
        ("--fork", "fork before continuing"),
        ("--share", "mark the session shareable"),
        ("--title <title>", "set the session title"),
        ("--session-root <path>", "session store root"),
    ];
    let model = [
        ("-m, --model <provider/model>", "provider/model override"),
        ("--provider <id>", "provider override"),
        ("--agent <name>", "agent profile to use"),
        ("--variant <name>", "provider-specific variant"),
        ("--wire-api <chat|responses>", "OpenAI-compatible wire API"),
        ("--base-url <url>", "provider base URL"),
    ];
    let input = [
        ("--command <name>", "render a custom command template"),
        ("-f, --file <path>", "attach a file; repeatable"),
        ("--mcp-config <path-or-json>", "enable remote MCP tools"),
        (
            "--answer <text>",
            "pre-answer a queued question; repeatable",
        ),
        ("--dir, --workspace <path>", "workspace path"),
    ];
    let runtime = [
        ("--stream", "emit provider deltas as they arrive"),
        ("--format <text|json|default>", "output format"),
        ("--thinking", "show thinking blocks when available"),
        ("--interactive, -i", "run direct interactive mode"),
        ("--permission <ruleset>", "FULL, READONLY, PLAN_ONLY, NONE"),
        (
            "--dangerously-skip-permissions",
            "auto-approve permissions that are not denied",
        ),
        ("--skip-doctor", "skip local gateway preflight"),
    ];
    let remote = [
        ("--attach <url>", "run through a remote App Bridge server"),
        ("--server-token <token>", "bearer token for --attach"),
        ("-u, --username <name>", "basic auth username"),
        ("-p, --password <password>", "basic auth password"),
    ];
    render_help_screen(
        "OpenAgent Run",
        "Start, resume, or attach an agent loop from the command line.",
        "openagent run [message..] [options]",
        &[
            HelpSection {
                title: "Session",
                rows: &session,
            },
            HelpSection {
                title: "Model And Agent",
                rows: &model,
            },
            HelpSection {
                title: "Input",
                rows: &input,
            },
            HelpSection {
                title: "Runtime",
                rows: &runtime,
            },
            HelpSection {
                title: "Remote Attach",
                rows: &remote,
            },
        ],
        &[
            "openagent run --stream \"fix the failing tests\"",
            "openagent run --agent reviewer --command review src/lib.rs",
            "openagent approval respond --decision allow_once && openagent run --continue",
        ],
    )
}

fn tui_help() -> &'static str {
    "Usage: openagent tui [options]\n\n\
     Options: --workspace <path>, --session-root <path>, -s/--session <id>, -c/--continue, --fork, --model <provider/model>, --agent <name>, --prompt <text>, --attach <url>, --server-token <token>, -u/--username <name>, -p/--password <password>, --skip-doctor"
}

fn serve_help() -> &'static str {
    "Usage: openagent serve [options]\n\n\
     Options: --host <host>, --port <port>, --workspace <path>, --session-root <path>, --headless, --auth-token <token>"
}

fn web_help() -> &'static str {
    "Usage: openagent web [options]\n\n\
     Options: --host <host>, --port <port>, --workspace <path>, --session-root <path>, --auth-token <token>"
}

fn client_help() -> &'static str {
    "Usage: openagent client [message..] [options]\n\n\
     Options: --server-url <url>, --server-token <token>, --workspace <path>, -s/--session <id>, -c/--continue, -f/--file <path>, --command <name>, --format <text|json>"
}

fn attach_help() -> &'static str {
    "Usage: openagent attach <url> [options]\n\n\
     Options: --workspace <path>, -s/--session <id>, -c/--continue, --fork, --format <text|json>, --skip-health-check, --server-token <token>, --server-token-env <name>, -u/--username <name>, -p/--password <password>"
}

fn doctor_help() -> &'static str {
    "Usage: openagent doctor [options]\n\n\
     Options: --format <text|json>, --base-url <url>, --model <id>, --wire-api <chat|responses>, --api-key <key>"
}

fn models_help() -> &'static str {
    "Usage: openagent models [provider] [options]\n\n\
     Options: --format <table|json>, --refresh, --offline, --catalog, --verbose, --ttl-seconds <n>, --models-url <url>"
}

fn session_help() -> &'static str {
    "Usage: openagent session <list|export|import|share|delete> [options]\n\n\
     list:   --session-root <path>, --format <table|json>, --max-count <n>\n\
     export: --session-root <path>, --sanitize [session_id]\n\
     import: --session-root <path> <file-or-url>\n\
     share:  --session-root <path> [session_id]\n\
     delete: --session-root <path> <session_id>"
}

fn stats_help() -> &'static str {
    "Usage: openagent stats [options]\n\n\
     Options: --session-root <path>, --days <n>, --format <table|json>"
}

fn command_help() -> &'static str {
    "Usage: openagent command <list|show|render> [options]\n\n\
     Options: --workspace <path>, --command-dir <path>, --format <table|json|text>"
}

fn config_help() -> &'static str {
    "Usage: openagent config <init|show> [options]\n\n\
     init: --workspace <path>, --path <file>, --api-key <key>, --base-url <url>, --model <id>, --wire-api <chat|responses>, --max-steps <n>, --with-server-token, --force, --format <text|json>\n\
     show: --workspace <path>, --session-root <path>, --server-url <url>, --format <table|json>"
}

fn auth_help(command_name: &str) -> String {
    format!(
        "Usage: openagent {command_name} <login|list|methods|logout> [options]\n\n\
         login: [provider-url] --provider <id> --api-key <key> --base-url <url> --model <id> --wire-api <chat|responses> --auth-file <file>\n\
         list: --auth-file <file> --format <table|json>\n\
         methods: [provider] --format <table|json>\n\
         logout: --provider <id> --auth-file <file>"
    )
}

fn mcp_help() -> &'static str {
    "Usage: openagent mcp <list|show|add|remove|auth|logout|doctor|debug> [options]\n\n\
     add: name --url <url> --transport <auto|http|sse> --header KEY=VALUE --timeout-ms <n> --disabled --config <file>\n\
     auth: list|status|login|set-token|callback\n\
     doctor/debug: --refresh --format <table|json>"
}

fn approval_help() -> &'static str {
    "Usage: openagent approval <list|respond|reject> [options]\n\n\
     Options: --session-root <path>, -s/--session <id>, --request-id <id>, --decision <allow_once|allow_always|reject>, --note <text>, --format <json|text>"
}

fn question_help() -> &'static str {
    "Usage: openagent question <list|reply|reject> [options]\n\n\
     Options: --session-root <path>, -s/--session <id>, --request-id <id>, --answer <text>, --format <json|text>"
}

fn client_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(client_help());
    }
    let server_url = value_for(args, &["--server-url"])
        .or_else(|| env::var("OPENAGENT_SERVER_URL").ok())
        .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string());
    let server_token = value_for(args, &["--server-token"])
        .or_else(|| env::var(DEFAULT_SERVER_TOKEN_ENV).ok())
        .or_else(|| value_for(args, &["--server-token-env"]).and_then(|name| env::var(name).ok()));
    let message = positional_args(
        args,
        &[
            "--server-url",
            "--server-token",
            "--server-token-env",
            "--workspace",
            "--dir",
            "--session",
            "-s",
            "--file",
            "-f",
            "--command",
            "--command-dir",
            "--format",
        ],
    )
    .join(" ");
    let workspace = workspace_from_args(args);
    let files = match attached_files(&workspace, &values_for(args, &["--file", "-f"])) {
        Ok(files) => files,
        Err(error) => return err_text(1, error),
    };
    let prompt = build_prompt_with_files(&message, &files);
    if prompt.trim().is_empty() {
        return err_text(2, "openagent client requires a prompt");
    }
    let session_id = match remote_select_session(
        &server_url,
        server_token.as_deref(),
        value_for(args, &["--session", "-s"]),
        has_flag(args, &["--continue", "-c"]),
        &workspace,
    ) {
        Ok(session_id) => session_id,
        Err(error) => return err_text(1, error),
    };
    let payload =
        match remote_start_turn(&server_url, server_token.as_deref(), &session_id, &prompt) {
            Ok(payload) => payload,
            Err(error) => return err_text(1, error),
        };
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        if let Some(events) = payload.get("events").and_then(Value::as_array) {
            return ok_text(
                events
                    .iter()
                    .map(python_json_dumps)
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        return CliRunResult::ok_json(&payload);
    }
    if let Some(events) = payload.get("events").and_then(Value::as_array) {
        ok_text(text_from_app_events(events))
    } else {
        ok_text(python_json_dumps(&payload))
    }
}

fn config_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(config_help());
    }
    match args[0].as_str() {
        "init" => config_init(&args[1..]),
        "show" => config_show(&args[1..]),
        _ => err_text(2, format!("unknown config command: {}", args[0])),
    }
}

fn config_init(args: &[String]) -> CliRunResult {
    let workspace = workspace_from_args(args);
    let path = value_for(args, &["--path"])
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join(".openagent/openagent.env"));
    if path.exists() && !has_flag(args, &["--force"]) {
        return err_text(1, format!("config file already exists: {}", path.display()));
    }
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return err_text(1, format!("failed to create config directory: {error}"));
    }
    let server_token =
        has_flag(args, &["--with-server-token"]).then(|| "generated-local-token".to_string());
    let lines = [
        value_for(args, &["--api-key"]).map(|value| format!("OPENAI_API_KEY={value}")),
        Some(format!(
            "OPENAI_BASE_URL={}",
            value_for(args, &["--base-url"]).unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
        )),
        Some(format!(
            "OPENAI_MODEL={}",
            value_for(args, &["--model"]).unwrap_or_else(|| DEFAULT_MODEL.to_string())
        )),
        Some(format!(
            "OPENAI_WIRE_API={}",
            value_for(args, &["--wire-api"]).unwrap_or_else(|| DEFAULT_WIRE_API.to_string())
        )),
        Some(format!(
            "OPENAGENT_APP_MAX_STEPS={}",
            value_for(args, &["--max-steps"]).unwrap_or_else(|| DEFAULT_MAX_STEPS.to_string())
        )),
        server_token.map(|token| format!("{DEFAULT_SERVER_TOKEN_ENV}={token}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    if let Err(error) = fs::write(&path, format!("{lines}\n")) {
        return err_text(1, format!("failed to write config: {error}"));
    }
    chmod_private(&path);
    let payload = json!({
        "created": true,
        "path": path.to_string_lossy(),
        "workspace": workspace.to_string_lossy(),
        "api_key_written": value_for(args, &["--api-key"]).is_some(),
        "server_token_written": has_flag(args, &["--with-server-token"]),
        "mode": "0o600",
        "next": ["openagent doctor", "openagent"],
    });
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!("created {}", path.display()))
    }
}

fn config_show(args: &[String]) -> CliRunResult {
    let workspace = workspace_from_args(args);
    let env_file = workspace.join(".openagent/openagent.env");
    let session_root = value_for(args, &["--session-root"])
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join(".openagent/sessions"));
    let payload = json!({
        "workspace": workspace.to_string_lossy(),
        "env_file": env_file.to_string_lossy(),
        "auth_file": auth_file_from_args(args).to_string_lossy(),
        "session_root": session_root.to_string_lossy(),
        "openai": {
            "base_url": env::var("OPENAI_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()),
            "model": env::var("OPENAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
            "wire_api": env::var("OPENAI_WIRE_API").unwrap_or_else(|_| DEFAULT_WIRE_API.to_string()),
            "api_key": if env::var("OPENAI_API_KEY").is_ok_and(|value| !value.is_empty()) {"set"} else {"missing"},
            "max_steps": env::var("OPENAGENT_APP_MAX_STEPS").unwrap_or_else(|_| DEFAULT_MAX_STEPS.to_string()),
        },
        "app_bridge": {
            "server_url": value_for(args, &["--server-url"]).unwrap_or_else(|| DEFAULT_SERVER_URL.to_string()),
            "server_token": if env::var(DEFAULT_SERVER_TOKEN_ENV).is_ok_and(|value| !value.is_empty()) {"set"} else {"missing"},
            "server_token_env": DEFAULT_SERVER_TOKEN_ENV,
        },
    });
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!(
            "workspace: {}\nenv_file: {}\nsession_root: {}",
            workspace.display(),
            env_file.display(),
            session_root.display()
        ))
    }
}

fn auth_command(command_name: &str, args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(auth_help(command_name));
    }
    match args[0].as_str() {
        "login" => auth_login(&args[1..]),
        "list" | "ls" => auth_list(&args[1..]),
        "methods" => auth_methods(&args[1..]),
        "logout" => auth_logout(&args[1..]),
        _ => err_text(2, format!("unknown {command_name} command: {}", args[0])),
    }
}

fn auth_login(args: &[String]) -> CliRunResult {
    let provider = value_for(args, &["--provider", "-p"]).unwrap_or_else(|| "openai".to_string());
    let provider = match normalize_provider(Some(&provider)) {
        Ok(provider) => provider,
        Err(error) => return err_text(2, error),
    };
    let api_key = value_for(args, &["--api-key"]).unwrap_or_default();
    let auth_file = auth_file_from_args(args);
    let mut auth = read_json_file(&auth_file);
    let providers = ensure_object_field(&mut auth, "providers");
    let base_url = value_for(args, &["--base-url"])
        .or_else(|| provider_default_base_url(&provider).ok().flatten())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let model = value_for(args, &["--model"])
        .or_else(|| provider_default_model(&provider).ok().flatten())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let wire_api = value_for(args, &["--wire-api"]).unwrap_or_else(|| DEFAULT_WIRE_API.to_string());
    providers.insert(
        provider.clone(),
        json!({
            "provider": provider,
            "type": value_for(args, &["--type"]).unwrap_or_else(|| "api".to_string()),
            "api_key": api_key,
            "base_url": base_url,
            "model": model,
            "wire_api": wire_api,
            "updated_at_ms": now_ms(),
        }),
    );
    let record = public_auth_record(
        &provider,
        providers.get(&provider).unwrap_or(&Value::Null),
        "auth_file",
    );
    if let Err(error) = write_json_file(&auth_file, &auth) {
        return err_text(1, error);
    }
    chmod_private(&auth_file);
    CliRunResult::ok_json(&json!({
        "status": "logged_in",
        "provider": provider,
        "auth_file": auth_file.to_string_lossy(),
        "record": record,
    }))
}

fn auth_list(args: &[String]) -> CliRunResult {
    let auth_file = auth_file_from_args(args);
    let auth = read_json_file(&auth_file);
    let providers = auth
        .get("providers")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .map(|(provider, value)| public_auth_record(provider, value, "auth_file"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let payload = json!({"auth_file": auth_file.to_string_lossy(), "providers": providers});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!(
            "{} provider(s)",
            payload["providers"].as_array().map_or(0, Vec::len)
        ))
    }
}

fn auth_methods(args: &[String]) -> CliRunResult {
    let provider = positional_args(args, &["--format"])
        .first()
        .cloned()
        .unwrap_or_else(active_provider);
    let present_env = env::vars().map(|(key, _)| key).collect::<BTreeSet<_>>();
    match provider_auth_methods(&provider, &present_env) {
        Ok(methods) => {
            let normalized = normalize_provider(Some(&provider)).unwrap_or(provider);
            let payload = json!({"provider": normalized, "methods": methods});
            if value_for(args, &["--format"]).as_deref() == Some("json") {
                CliRunResult::ok_json(&payload)
            } else {
                ok_text(format!(
                    "{} auth method(s)",
                    payload["methods"].as_array().map_or(0, Vec::len)
                ))
            }
        }
        Err(error) => err_text(2, error),
    }
}

fn auth_logout(args: &[String]) -> CliRunResult {
    let provider = value_for(args, &["--provider", "-p"]).unwrap_or_else(|| "openai".to_string());
    let provider = match normalize_provider(Some(&provider)) {
        Ok(provider) => provider,
        Err(error) => return err_text(2, error),
    };
    let auth_file = auth_file_from_args(args);
    let mut auth = read_json_file(&auth_file);
    let removed = auth
        .get_mut("providers")
        .and_then(Value::as_object_mut)
        .and_then(|providers| providers.remove(&provider))
        .is_some();
    if let Err(error) = write_json_file(&auth_file, &auth) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"provider": provider, "removed": removed, "auth_file": auth_file.to_string_lossy()}),
    )
}

fn approval_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(approval_help());
    }
    match args[0].as_str() {
        "list" | "ls" => pending_request_list(args, "pending_approval"),
        "respond" | "allow" | "approve" => approval_respond(&args[1..], false),
        "reject" | "deny" => approval_respond(&args[1..], true),
        other => err_text(2, format!("unknown approval command: {other}")),
    }
}

fn question_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(question_help());
    }
    match args[0].as_str() {
        "list" | "ls" => pending_request_list(args, "pending_question"),
        "reply" | "answer" => question_reply(&args[1..], false),
        "reject" | "dismiss" => question_reply(&args[1..], true),
        other => err_text(2, format!("unknown question command: {other}")),
    }
}

fn pending_request_list(args: &[String], key: &str) -> CliRunResult {
    let root = session_root_from_args(args);
    let Some(session_id) =
        value_for(args, &["--session", "-s"]).or_else(|| latest_session_id(&root))
    else {
        return err_text(1, "No sessions found");
    };
    let store = FileSessionStore::new(root);
    let session = match store.load_session(&session_id) {
        Ok(session) => session,
        Err(error) => return err_text(1, format!("failed to load session: {error}")),
    };
    let pending = session.metadata.get(key).cloned().unwrap_or(Value::Null);
    CliRunResult::ok_json(&json!({
        "session_id": session.id,
        "kind": key.trim_start_matches("pending_"),
        "pending": pending,
    }))
}

fn approval_respond(args: &[String], reject: bool) -> CliRunResult {
    let root = session_root_from_args(args);
    let mut session = match load_response_session(args, &root) {
        Ok(session) => session,
        Err(error) => return err_text(1, error),
    };
    let Some(pending) = session.metadata.get("pending_approval").cloned() else {
        return err_text(1, "No pending approval request in session");
    };
    if let Err(error) = ensure_request_matches(args, &pending) {
        return err_text(2, error);
    }
    let decision = if reject || has_flag(args, &["--reject", "--deny"]) {
        "reject".to_string()
    } else if has_flag(args, &["--always"]) {
        "allow_always".to_string()
    } else {
        value_for(args, &["--decision"]).unwrap_or_else(|| "allow_once".to_string())
    };
    if !matches!(
        decision.as_str(),
        "allow_once" | "allow_always" | "always" | "reject" | "deny"
    ) {
        return err_text(
            2,
            "approval decision must be allow_once, allow_always, or reject",
        );
    }
    let response = json!({
        "request_id": pending.get("request_id").cloned().unwrap_or(Value::Null),
        "decision": decision,
        "note": value_for(args, &["--note"]),
        "updated_at_ms": now_ms_cli(),
    });
    session
        .metadata
        .insert("pending_approval_response".to_string(), response.clone());
    session.status = SessionStatus::Paused;
    let store = FileSessionStore::new(root);
    let run_id = pending.get("run_id").and_then(Value::as_str);
    if let Err(error) = store.save_state(&session, run_id) {
        return err_text(1, format!("failed to save approval response: {error}"));
    }
    CliRunResult::ok_json(&json!({
        "session_id": session.id,
        "queued": true,
        "response": response,
        "next": format!("openagent run --continue --session-root {}", store.root.to_string_lossy()),
    }))
}

fn question_reply(args: &[String], reject: bool) -> CliRunResult {
    let root = session_root_from_args(args);
    let mut session = match load_response_session(args, &root) {
        Ok(session) => session,
        Err(error) => return err_text(1, error),
    };
    let Some(pending) = session.metadata.get("pending_question").cloned() else {
        return err_text(1, "No pending question request in session");
    };
    if let Err(error) = ensure_request_matches(args, &pending) {
        return err_text(2, error);
    }
    let answers = if reject {
        Vec::<Vec<String>>::new()
    } else {
        let mut answers = values_for(args, &["--answer"])
            .into_iter()
            .map(|answer| split_answer_items(&answer))
            .collect::<Vec<_>>();
        if answers.is_empty() {
            let positionals = positional_args(
                args,
                &[
                    "--workspace",
                    "--dir",
                    "--session-root",
                    "--session",
                    "-s",
                    "--request-id",
                    "--format",
                ],
            );
            if !positionals.is_empty() {
                answers.push(vec![positionals.join(" ")]);
            }
        }
        if answers.is_empty() {
            return err_text(2, "question reply requires --answer or answer text");
        }
        answers
    };
    let response = json!({
        "request_id": pending.get("request_id").cloned().unwrap_or(Value::Null),
        "decision": if reject { "reject" } else { "reply" },
        "answers": answers,
        "updated_at_ms": now_ms_cli(),
    });
    session
        .metadata
        .insert("pending_question_response".to_string(), response.clone());
    session.status = SessionStatus::Paused;
    let store = FileSessionStore::new(root);
    let run_id = pending.get("run_id").and_then(Value::as_str);
    if let Err(error) = store.save_state(&session, run_id) {
        return err_text(1, format!("failed to save question response: {error}"));
    }
    CliRunResult::ok_json(&json!({
        "session_id": session.id,
        "queued": true,
        "response": response,
        "next": format!("openagent run --continue --session-root {}", store.root.to_string_lossy()),
    }))
}

fn load_response_session(args: &[String], root: &Path) -> Result<Session, String> {
    let session_id = value_for(args, &["--session", "-s"])
        .or_else(|| latest_session_id(root))
        .ok_or_else(|| "No sessions found".to_string())?;
    if !valid_session_id(&session_id) {
        return Err("Invalid session id".to_string());
    }
    FileSessionStore::new(root)
        .load_session(&session_id)
        .map_err(|error| format!("failed to load session: {error}"))
}

fn ensure_request_matches(args: &[String], pending: &Value) -> Result<(), String> {
    let expected = pending
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let requested = value_for(args, &["--request-id"]).or_else(|| {
        positional_args(
            args,
            &[
                "--workspace",
                "--dir",
                "--session-root",
                "--session",
                "-s",
                "--decision",
                "--note",
                "--answer",
                "--format",
            ],
        )
        .first()
        .cloned()
        .filter(|value| value.starts_with("approval_") || value.starts_with("question_"))
    });
    if let Some(requested) = requested
        && !expected.is_empty()
        && requested != expected
    {
        return Err(format!(
            "request id mismatch: expected {expected}, received {requested}"
        ));
    }
    Ok(())
}

fn session_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(session_help());
    }
    match args[0].as_str() {
        "list" | "ls" => session_list(&args[1..]),
        "export" => session_export(&args[1..]),
        "import" => session_import(&args[1..]),
        "share" => session_share(&args[1..]),
        "delete" | "rm" => session_delete(&args[1..]),
        _ => err_text(2, format!("unknown session command: {}", args[0])),
    }
}

fn session_list(args: &[String]) -> CliRunResult {
    let root = session_root_from_args(args);
    let mut sessions = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let state = read_json_file(&path.join("state.latest.json"));
            if state.as_object().is_none_or(Map::is_empty) {
                continue;
            }
            let fallback_id = entry.file_name().to_string_lossy().to_string();
            sessions.push(json!({
                "session_id": state.get("session_id").and_then(Value::as_str).unwrap_or(&fallback_id),
                "workspace": state.get("workspace").cloned().unwrap_or_else(|| json!(".")),
                "status": state.get("status").cloned().unwrap_or_else(|| json!("idle")),
                "updated_at_ms": state.get("updated_at_ms").cloned().unwrap_or_else(|| json!(0)),
                "message_count": state.get("messages").and_then(Value::as_array).map_or(0, Vec::len),
            }));
        }
    }
    sessions.sort_by(|left, right| {
        right["updated_at_ms"]
            .as_u64()
            .cmp(&left["updated_at_ms"].as_u64())
    });
    if let Some(max_count) =
        value_for(args, &["--max-count", "-n"]).and_then(|value| value.parse::<usize>().ok())
    {
        sessions.truncate(max_count);
    }
    let payload = json!({"session_root": root.to_string_lossy(), "sessions": sessions});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        let sessions = payload["sessions"].as_array().cloned().unwrap_or_default();
        let mut text = render_key_values(
            "Sessions",
            &[
                ("Root", root.to_string_lossy().to_string()),
                ("Count", sessions.len().to_string()),
            ],
        );
        if !sessions.is_empty() {
            let rows = sessions
                .iter()
                .map(|session| {
                    vec![
                        session
                            .get("session_id")
                            .and_then(Value::as_str)
                            .unwrap_or("-")
                            .to_string(),
                        session
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("idle")
                            .to_string(),
                        session
                            .get("message_count")
                            .and_then(Value::as_u64)
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "0".to_string()),
                        session
                            .get("workspace")
                            .map(compact_text_value)
                            .unwrap_or_else(|| ".".to_string()),
                    ]
                })
                .collect::<Vec<_>>();
            text.push_str("\n\n");
            text.push_str(&render_table(
                &["Session", "Status", "Messages", "Workspace"],
                &rows,
            ));
        }
        ok_text(text)
    }
}

fn session_export(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--session-root"]);
    let root = session_root_from_args(args);
    let session_id = if let Some(session_id) = positionals.first() {
        session_id.clone()
    } else {
        match latest_session_id(&root) {
            Some(session_id) => session_id,
            None => return err_text(2, "session export requires a session id"),
        }
    };
    if !valid_session_id(&session_id) {
        return err_text(2, "Invalid session id");
    }
    let state_path = root.join(&session_id).join("state.latest.json");
    let mut state = read_json_file(&state_path);
    if state.as_object().is_none_or(Map::is_empty) {
        return err_text(1, format!("Session state not found: {session_id}"));
    }
    if has_flag(args, &["--sanitize"]) {
        sanitize_session_state(&mut state);
    }
    CliRunResult::ok_json(
        &json!({"schema_version": "openagent.session_export.v1", "session": state}),
    )
}

fn session_import(args: &[String]) -> CliRunResult {
    let positionals = positional_args(
        args,
        &["--workspace", "--dir", "--session-root", "--format"],
    );
    let Some(source) = positionals.first() else {
        return err_text(2, "session import requires a file or URL");
    };
    match import_session_source(&session_root_from_args(args), source) {
        Ok(payload) => CliRunResult::ok_json(&payload),
        Err(error) => err_text(1, error),
    }
}

fn session_share(args: &[String]) -> CliRunResult {
    let positionals = positional_args(
        args,
        &["--workspace", "--dir", "--session-root", "--format"],
    );
    let root = session_root_from_args(args);
    let session_id = if let Some(session_id) = positionals.first() {
        session_id.clone()
    } else {
        match latest_session_id(&root) {
            Some(session_id) => session_id,
            None => return err_text(2, "session share requires a session id"),
        }
    };
    let store = FileSessionStore::new(root);
    match share_session(&store, &session_id, has_flag(args, &["--sanitize"])) {
        Ok(payload) => CliRunResult::ok_json(&payload),
        Err(error) => err_text(1, error),
    }
}

fn session_delete(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--session-root"]);
    let Some(session_id) = positionals.first() else {
        return err_text(2, "session delete requires a session id");
    };
    if !valid_session_id(session_id) {
        return err_text(2, "Invalid session id");
    }
    let target = session_root_from_args(args).join(session_id);
    let removed = if target.exists() {
        match fs::remove_dir_all(&target) {
            Ok(()) => true,
            Err(error) => return err_text(1, format!("failed to delete session: {error}")),
        }
    } else {
        false
    };
    CliRunResult::ok_json(&json!({"session_id": session_id, "removed": removed}))
}

fn stats_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(stats_help());
    }
    let root = session_root_from_args(args);
    let mut session_count = 0_u64;
    let mut run_count = 0_u64;
    let mut input = 0_u64;
    let mut output = 0_u64;
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let state = read_json_file(&path.join("state.latest.json"));
            if state.as_object().is_none_or(Map::is_empty) {
                continue;
            }
            session_count += 1;
            let runs = path.join("runs");
            if let Ok(run_entries) = fs::read_dir(runs) {
                for run_entry in run_entries.flatten() {
                    let summary = read_json_file(&run_entry.path().join("summary.json"));
                    if summary.as_object().is_some_and(|object| !object.is_empty()) {
                        run_count += 1;
                        input += summary
                            .get("total_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        output += summary
                            .get("total_output_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                    }
                }
            }
        }
    }
    let payload = json!({"session_root": root.to_string_lossy(), "session_count": session_count, "run_count": run_count, "total_input_tokens": input, "total_output_tokens": output});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(render_key_values(
            "Usage Stats",
            &[
                ("Session Root", root.to_string_lossy().to_string()),
                ("Sessions", session_count.to_string()),
                ("Runs", run_count.to_string()),
                ("Input Tokens", input.to_string()),
                ("Output Tokens", output.to_string()),
            ],
        ))
    }
}

fn custom_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(command_help());
    }
    match args[0].as_str() {
        "list" | "ls" => custom_command_list(&args[1..]),
        "show" => custom_command_show(&args[1..]),
        "render" => custom_command_render(&args[1..]),
        _ => err_text(2, format!("unknown command action: {}", args[0])),
    }
}

fn custom_command_list(args: &[String]) -> CliRunResult {
    let commands = discover_custom_commands(args);
    let payload =
        json!({"commands": commands.iter().map(|item| item.public(false)).collect::<Vec<_>>()});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!("{} command(s)", commands.len()))
    }
}

fn custom_command_show(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--command-dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "command show requires a name");
    };
    let Some(command) = discover_custom_commands(args)
        .into_iter()
        .find(|item| item.name == *name)
    else {
        return err_text(1, format!("Command not found: {name}"));
    };
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&command.public(true))
    } else {
        ok_text(command.template)
    }
}

fn custom_command_render(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--command-dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "command render requires a name");
    };
    let command_args = positionals.iter().skip(1).cloned().collect::<Vec<_>>();
    let workspace = workspace_from_args(args);
    let Some(command) = discover_custom_commands(args)
        .into_iter()
        .find(|item| item.name == *name)
    else {
        return err_text(1, format!("Command not found: {name}"));
    };
    let rendered = render_custom_template(&command.template, &command_args, &workspace);
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&json!({"command": command.public(false), "prompt": rendered}))
    } else {
        ok_text(rendered)
    }
}

fn latest_session_id(root: &Path) -> Option<String> {
    let mut sessions = fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let state = read_json_file(&path.join("state.latest.json"));
            let id = state
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| entry.file_name().to_string_lossy().to_string());
            let updated = state
                .get("updated_at_ms")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            Some((updated, id))
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|item| std::cmp::Reverse(item.0));
    sessions.into_iter().map(|(_, id)| id).next()
}

fn share_session(
    store: &FileSessionStore,
    session_id: &str,
    sanitize: bool,
) -> Result<Value, String> {
    if !valid_session_id(session_id) {
        return Err("Invalid session id".to_string());
    }
    let state_path = store.root.join(session_id).join("state.latest.json");
    let mut state = read_json_file(&state_path);
    if state.as_object().is_none_or(Map::is_empty) {
        return Err(format!("Session state not found: {session_id}"));
    }
    if sanitize {
        sanitize_session_state(&mut state);
    }
    let share_dir = store.root.join("shares");
    fs::create_dir_all(&share_dir).map_err(|error| error.to_string())?;
    let share_id = new_cli_id("share");
    let path = share_dir.join(format!("{share_id}.json"));
    let payload = json!({
        "schema_version": "openagent.session_share.v1",
        "share_id": share_id,
        "session": state,
    });
    write_json_file(&path, &payload)?;
    Ok(json!({
        "share_id": payload["share_id"],
        "session_id": session_id,
        "path": path.to_string_lossy(),
        "url": format!("file://{}", path.to_string_lossy()),
    }))
}

fn import_session_source(root: &Path, source: &str) -> Result<Value, String> {
    let raw = if source.starts_with("http://") || source.starts_with("https://") {
        reqwest::blocking::get(source)
            .map_err(|error| format!("failed to fetch import source: {error}"))?
            .text()
            .map_err(|error| format!("failed to read import response: {error}"))?
    } else {
        fs::read_to_string(source)
            .map_err(|error| format!("failed to read import file: {error}"))?
    };
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("import source was not JSON: {error}"))?;
    let session = value
        .get("session")
        .cloned()
        .or_else(|| value.get("data").and_then(|data| data.get("session")).cloned())
        .or_else(|| {
            value
                .get("info")
                .map(|info| json!({"session_id": info.get("id").cloned().unwrap_or_else(|| json!(new_cli_id("session"))), "workspace": info.get("directory").cloned().unwrap_or_else(|| json!(".")), "status": "idle", "updated_at_ms": now_ms_cli(), "messages": value.get("messages").cloned().unwrap_or_else(|| json!([])), "metadata": {"imported_from": source}}))
        })
        .ok_or_else(|| "import source does not contain a session".to_string())?;
    let session_id = session
        .get("session_id")
        .or_else(|| session.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| new_cli_id("session"));
    if !valid_session_id(&session_id) {
        return Err("Invalid session id in import".to_string());
    }
    let target = root.join(&session_id);
    fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    let mut state = session;
    if let Some(object) = state.as_object_mut() {
        object.insert("session_id".to_string(), json!(session_id.clone()));
        object
            .entry("updated_at_ms".to_string())
            .or_insert_with(|| json!(now_ms_cli()));
        object
            .entry("schema_version".to_string())
            .or_insert_with(|| json!("openagent.session_state.v1"));
    }
    write_json_file(&target.join("state.latest.json"), &state)?;
    write_json_file(
        &target.join("session.json"),
        &json!({
            "schema_version": "openagent.session.v1",
            "session_id": session_id,
            "workspace": state.get("workspace").cloned().unwrap_or_else(|| json!(".")),
            "status": state.get("status").cloned().unwrap_or_else(|| json!("idle")),
            "created_at_ms": now_ms_cli(),
            "updated_at_ms": now_ms_cli(),
        }),
    )?;
    Ok(json!({"imported": true, "session_id": session_id, "session_root": root.to_string_lossy()}))
}

fn github_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent github <status|issue|pr|workflow> [args...]");
    }
    match args[0].as_str() {
        "status" => run_external_json("gh", &["status"]),
        "issue" => github_issue_command(&args[1..]),
        "pr" => run_external_json(
            "gh",
            &[
                "pr",
                "list",
                "--limit",
                "20",
                "--json",
                "number,title,state,url,headRefName",
            ],
        ),
        "workflow" | "worktree" | "start" => github_workflow_command(&args[1..]),
        other => err_text(2, format!("unknown github command: {other}")),
    }
}

fn pr_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent pr <list|view|checkout|template|review> [number]");
    }
    match args[0].as_str() {
        "list" | "ls" => run_external_json(
            "gh",
            &[
                "pr",
                "list",
                "--limit",
                "20",
                "--json",
                "number,title,state,url,headRefName",
            ],
        ),
        "view" => {
            let Some(number) = args.get(1) else {
                return err_text(2, "pr view requires a number");
            };
            run_external_json(
                "gh",
                &[
                    "pr",
                    "view",
                    number,
                    "--json",
                    "number,title,state,url,headRefName,body,reviewDecision",
                ],
            )
        }
        "checkout" => {
            let Some(number) = args.get(1).or_else(|| args.first()) else {
                return err_text(2, "pr checkout requires a number");
            };
            run_external_json("gh", &["pr", "checkout", number])
        }
        "template" | "review" => {
            let number = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| "<number>".to_string());
            CliRunResult::ok_json(&json!({
                "schema_version": "openagent.pr_review.v1",
                "number": number,
                "checklist": [
                    "summarize intent and changed files",
                    "run tests or inspect CI",
                    "review behavior regressions and missing tests",
                    "write actionable findings with file/line references"
                ],
                "commands": [
                    format!("gh pr view {number} --json files,comments,reviews,checks"),
                    format!("gh pr diff {number}"),
                ]
            }))
        }
        number => run_external_json("gh", &["pr", "checkout", number]),
    }
}

fn debug_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent debug <info|paths|env|sessions|file|rg|bundle>");
    }
    match args[0].as_str() {
        "info" => CliRunResult::ok_json(&json!({
            "version": env!("CARGO_PKG_VERSION"),
            "cwd": env::current_dir().ok().map(|path| path.to_string_lossy().to_string()),
            "provider": active_provider(),
            "session_root": session_root_from_args(args).to_string_lossy(),
        })),
        "paths" => CliRunResult::ok_json(&json!({
            "home": home_dir().to_string_lossy(),
            "models_cache": models_cache_path().to_string_lossy(),
            "auth_file": auth_file_from_args(args).to_string_lossy(),
            "mcp_config": mcp_config_path(args).to_string_lossy(),
            "session_root": session_root_from_args(args).to_string_lossy(),
        })),
        "env" => CliRunResult::ok_json(&json!({"env": sanitized_env()})),
        "sessions" => session_list(&args[1..]),
        "bundle" => debug_bundle(args),
        "file" => {
            let Some(path) = args.get(1) else {
                return err_text(2, "debug file requires a path");
            };
            match fs::read_to_string(path) {
                Ok(text) => ok_text(text),
                Err(error) => err_text(1, error.to_string()),
            }
        }
        "rg" => {
            let Some(pattern) = args.get(1) else {
                return err_text(2, "debug rg requires a pattern");
            };
            run_external_json("rg", &[pattern])
        }
        other => err_text(2, format!("unknown debug command: {other}")),
    }
}

fn db_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent db <path|summary|rebuild|query|schema|export-sql>");
    }
    match args.first().map(String::as_str).unwrap_or("summary") {
        "path" => ok_text(
            session_root_from_args(args)
                .join("index.jsonl")
                .to_string_lossy(),
        ),
        "summary" => stats_command(args),
        "rebuild" => db_rebuild(args),
        "query" => db_query(args),
        "schema" => ok_text(db_schema_sql()),
        "export-sql" => db_export_sql(args),
        other => err_text(2, format!("unknown db command: {other}")),
    }
}

fn lifecycle_command(name: &str, args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(format!("Usage: openagent {name} [--dry-run]"));
    }
    CliRunResult::ok_json(&json!({
        "command": name,
        "performed": false,
        "dry_run": !has_flag(args, &["--yes"]),
        "version": env!("CARGO_PKG_VERSION"),
        "repository": env!("CARGO_PKG_REPOSITORY"),
        "binary": std::env::current_exe().ok().map(|path| path.to_string_lossy().to_string()),
        "plan": lifecycle_plan(name),
        "reason": "OpenAgent is source-tree managed in this workspace; destructive lifecycle changes require --yes and a distribution package.",
    }))
}

fn acp_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(
            "Usage: openagent acp <manifest|serve> [--host <host>] [--port <port>] [--cwd <path>]",
        );
    }
    if args.first().map(String::as_str).unwrap_or("manifest") == "manifest" {
        return CliRunResult::ok_json(&json!({
            "schema_version": "openagent.acp_manifest.v1",
            "commands": ["session/new", "session/list", "turn/start", "turn/interrupt", "approval/respond", "question/reply"],
            "transport": {"http": "/api", "sse": "/api/events"},
            "auth": ["bearer", "basic"],
        }));
    }
    let mut runtime_args = args.to_vec();
    if runtime_args.first().map(String::as_str) == Some("serve") {
        runtime_args.remove(0);
    }
    runtime_args.push("--headless".to_string());
    http_runtime_command(&runtime_args, false, "Usage: openagent acp")
}

fn generate_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent generate <openapi|commands|acp>");
    }
    if args.first().map(String::as_str) == Some("commands") {
        return CliRunResult::ok_json(&json!({
            "schema_version": "openagent.commands.v1",
            "commands": ["run", "tui", "serve", "web", "models", "agent", "plugin", "github", "pr", "debug", "db", "acp", "generate", "console"],
        }));
    }
    if args.first().map(String::as_str) == Some("acp") {
        return acp_command(&["manifest".to_string()]);
    }
    CliRunResult::ok_json(&json!({
        "openapi": "3.1.0",
        "info": {"title": "OpenAgent App Bridge", "version": env!("CARGO_PKG_VERSION")},
        "paths": {
            "/api/health": {"get": {"operationId": "health"}},
            "/api/events": {"get": {"operationId": "globalEvents"}},
            "/api/sessions": {"get": {"operationId": "listSessions"}, "post": {"operationId": "createSession"}},
            "/api/sessions/{session_id}/turns": {"post": {"operationId": "startTurn"}},
            "/api/turns/{turn_id}/events": {"get": {"operationId": "turnEvents"}},
            "/api/turns/{turn_id}/interrupt": {"post": {"operationId": "interruptTurn"}},
            "/tui/control/next": {"get": {"operationId": "nextTuiControl"}}
        }
    }))
}

fn console_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent console <login|logout|whoami|orgs|open>");
    }
    let path = home_dir().join(".config/openagent/console.json");
    match args[0].as_str() {
        "login" => {
            let url = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| "https://app.openagent.local".to_string());
            let payload = json!({
                "url": url,
                "profile": {"login": env::var("USER").unwrap_or_else(|_| "local".to_string())},
                "orgs": [{"login": "local", "role": "owner"}],
                "updated_at_ms": now_ms_cli(),
            });
            if let Err(error) = write_json_file(&path, &payload) {
                return err_text(1, error);
            }
            CliRunResult::ok_json(&json!({"logged_in": true, "path": path.to_string_lossy()}))
        }
        "logout" => {
            let removed = fs::remove_file(&path).is_ok();
            CliRunResult::ok_json(&json!({"logged_out": removed}))
        }
        "whoami" | "orgs" | "open" => CliRunResult::ok_json(&read_json_file(&path)),
        other => err_text(2, format!("unknown console command: {other}")),
    }
}

fn github_issue_command(args: &[String]) -> CliRunResult {
    match args.first().map(String::as_str).unwrap_or("list") {
        "list" | "ls" => run_external_json(
            "gh",
            &[
                "issue",
                "list",
                "--limit",
                "20",
                "--json",
                "number,title,state,url,labels,assignees",
            ],
        ),
        "view" => {
            let Some(number) = args.get(1) else {
                return err_text(2, "github issue view requires a number");
            };
            run_external_json(
                "gh",
                &[
                    "issue",
                    "view",
                    number,
                    "--json",
                    "number,title,state,url,body,labels,assignees",
                ],
            )
        }
        "start" => github_workflow_command(&args[1..]),
        other => err_text(2, format!("unknown github issue command: {other}")),
    }
}

fn github_workflow_command(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--format", "--title"]);
    let issue = positionals
        .iter()
        .find(|value| value.chars().any(|item| item.is_ascii_alphanumeric()))
        .cloned()
        .unwrap_or_else(|| value_for(args, &["--title"]).unwrap_or_else(|| "manual".to_string()));
    let workflow_id = format!("workflow_{}", sanitize_identifier(&issue));
    let path = workspace_from_args(args)
        .join(".openagent/github/workflows")
        .join(format!("{workflow_id}.json"));
    let branch = format!("openagent/{}", sanitize_identifier(&issue));
    let payload = json!({
        "schema_version": "openagent.github_workflow.v1",
        "id": workflow_id,
        "issue": issue,
        "branch": branch,
        "status": "planned",
        "steps": [
            "inspect issue and repository state",
            "create or switch to the workflow branch",
            "implement the smallest verified slice",
            "run tests and capture evidence",
            "open or update a pull request"
        ],
        "commands": [
            format!("git switch -c {branch}"),
            "openagent run --skip-doctor \"implement issue scope\"".to_string(),
            "gh pr create --draft".to_string()
        ],
        "created_at_ms": now_ms_cli(),
    });
    if let Err(error) = write_json_file(&path, &payload) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"created": true, "path": path.to_string_lossy(), "workflow": payload}),
    )
}

fn debug_bundle(args: &[String]) -> CliRunResult {
    let path = workspace_from_args(args)
        .join(".openagent/debug")
        .join(format!("bundle_{}.json", now_ms_cli()));
    let payload = json!({
        "schema_version": "openagent.debug_bundle.v1",
        "info": {
            "version": env!("CARGO_PKG_VERSION"),
            "cwd": env::current_dir().ok().map(|path| path.to_string_lossy().to_string()),
            "provider": active_provider(),
        },
        "paths": {
            "models_cache": models_cache_path().to_string_lossy(),
            "auth_file": auth_file_from_args(args).to_string_lossy(),
            "session_root": session_root_from_args(args).to_string_lossy(),
        },
        "env": sanitized_env(),
        "stats": stats_payload(&session_root_from_args(args)),
        "created_at_ms": now_ms_cli(),
    });
    if let Err(error) = write_json_file(&path, &payload) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(&json!({"path": path.to_string_lossy(), "bundle": payload}))
}

fn db_rebuild(args: &[String]) -> CliRunResult {
    let root = session_root_from_args(args);
    let rows = session_db_rows(&root);
    let index_path = root.join("index.jsonl");
    if let Some(parent) = index_path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return err_text(1, error.to_string());
    }
    let mut raw = String::new();
    for row in &rows {
        raw.push_str(&python_json_dumps(row));
        raw.push('\n');
    }
    if let Err(error) = fs::write(&index_path, raw) {
        return err_text(1, error.to_string());
    }
    CliRunResult::ok_json(
        &json!({"rebuilt": true, "path": index_path.to_string_lossy(), "rows": rows.len()}),
    )
}

fn db_query(args: &[String]) -> CliRunResult {
    let root = session_root_from_args(args);
    let query = value_for(args, &["--match", "-m"]).or_else(|| {
        positional_args(
            args,
            &["--workspace", "--dir", "--session-root", "--format"],
        )
        .get(1)
        .cloned()
    });
    let rows = session_db_rows(&root)
        .into_iter()
        .filter(|row| {
            query.as_ref().is_none_or(|needle| {
                python_json_dumps(row)
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            })
        })
        .collect::<Vec<_>>();
    CliRunResult::ok_json(&json!({"session_root": root.to_string_lossy(), "rows": rows}))
}

fn db_export_sql(args: &[String]) -> CliRunResult {
    let root = session_root_from_args(args);
    let path = value_for(args, &["--output", "-o"])
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("index.sql"));
    let mut sql = db_schema_sql();
    for row in session_db_rows(&root) {
        sql.push_str(&format!(
            "INSERT INTO sessions(session_id, workspace, status, updated_at_ms, message_count, run_count) VALUES('{}', '{}', '{}', {}, {}, {});\n",
            sql_escape(row.get("session_id").and_then(Value::as_str).unwrap_or_default()),
            sql_escape(row.get("workspace").and_then(Value::as_str).unwrap_or_default()),
            sql_escape(row.get("status").and_then(Value::as_str).unwrap_or_default()),
            row.get("updated_at_ms").and_then(Value::as_u64).unwrap_or_default(),
            row.get("message_count").and_then(Value::as_u64).unwrap_or_default(),
            row.get("run_count").and_then(Value::as_u64).unwrap_or_default(),
        ));
    }
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return err_text(1, error.to_string());
    }
    if let Err(error) = fs::write(&path, sql) {
        return err_text(1, error.to_string());
    }
    CliRunResult::ok_json(&json!({"path": path.to_string_lossy(), "exported": true}))
}

fn db_schema_sql() -> String {
    "CREATE TABLE IF NOT EXISTS sessions(session_id TEXT PRIMARY KEY, workspace TEXT, status TEXT, updated_at_ms INTEGER, message_count INTEGER, run_count INTEGER);\nCREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at_ms DESC);\n".to_string()
}

fn session_db_rows(root: &Path) -> Vec<Value> {
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let state = read_json_file(&path.join("state.latest.json"));
            if state.as_object().is_none_or(Map::is_empty) {
                continue;
            }
            let fallback_id = entry.file_name().to_string_lossy().to_string();
            let run_count = fs::read_dir(path.join("runs"))
                .ok()
                .map(|items| items.flatten().filter(|item| item.path().is_dir()).count())
                .unwrap_or_default();
            rows.push(json!({
                "session_id": state.get("session_id").and_then(Value::as_str).unwrap_or(&fallback_id),
                "workspace": state.get("workspace").cloned().unwrap_or_else(|| json!(".")),
                "status": state.get("status").cloned().unwrap_or_else(|| json!("idle")),
                "updated_at_ms": state.get("updated_at_ms").cloned().unwrap_or_else(|| json!(0)),
                "message_count": state.get("messages").and_then(Value::as_array).map_or(0, Vec::len),
                "run_count": run_count,
            }));
        }
    }
    rows.sort_by(|left, right| {
        right["updated_at_ms"]
            .as_u64()
            .cmp(&left["updated_at_ms"].as_u64())
    });
    rows
}

fn stats_payload(root: &Path) -> Value {
    let mut session_count = 0_u64;
    let mut run_count = 0_u64;
    for row in session_db_rows(root) {
        session_count += 1;
        run_count += row
            .get("run_count")
            .and_then(Value::as_u64)
            .unwrap_or_default();
    }
    json!({"session_root": root.to_string_lossy(), "session_count": session_count, "run_count": run_count})
}

fn lifecycle_plan(name: &str) -> Value {
    match name {
        "upgrade" => json!([
            "inspect current binary and repository",
            "fetch latest release or git remote",
            "run cargo build/test after upgrade",
            "replace binary only after verification"
        ]),
        "uninstall" => json!([
            "locate binary and config/cache directories",
            "show files that would be removed",
            "require explicit --yes before destructive removal"
        ]),
        _ => json!([]),
    }
}

fn sanitized_env() -> Value {
    let mut envs = Map::new();
    for (key, value) in env::vars() {
        if key.starts_with("OPENAGENT_")
            || key.starts_with("OPENAI_")
            || key.starts_with("ANTHROPIC_")
            || key.ends_with("_API_KEY")
        {
            envs.insert(
                key,
                if looks_secret(&value) {
                    json!(mask_secret(&value))
                } else {
                    json!(value)
                },
            );
        }
    }
    Value::Object(envs)
}

fn sql_escape(value: &str) -> String {
    value.replace('\'', "''")
}

fn run_external_json(program: &str, args: &[&str]) -> CliRunResult {
    match Command::new(program).args(args).output() {
        Ok(output) => CliRunResult {
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(error) => err_text(1, format!("failed to run {program}: {error}")),
    }
}

fn chat_message(role: Role, content: String) -> ChatMessage {
    ChatMessage {
        role,
        content,
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::from([("message_id".to_string(), json!(new_cli_id("msg")))]),
    }
}

fn cli_message_id(index: u64) -> String {
    format!("msg_{index}")
}

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}

fn now_ms_cli() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn new_cli_id(prefix: &str) -> String {
    format!("{prefix}_{}_{}", now_ms_cli(), std::process::id())
}

#[allow(dead_code)]
fn opencode_gap_command(command: &str, args: &[String]) -> CliRunResult {
    let help = format!(
        "openagent {command} is tracked as an OpenCode parity backlog command.\n\
         The Rust rewrite exposes this boundary, but full behavior is not implemented yet."
    );
    if args.iter().any(|arg| is_help_flag(arg)) {
        ok_text(help)
    } else {
        err_text(2, help)
    }
}

fn active_provider() -> String {
    env::var("OPENAGENT_PROVIDER")
        .or_else(|_| env::var("OPENAGENT_ACTIVE_PROVIDER"))
        .unwrap_or_else(|_| "openai".to_string())
}

fn provider_env_value(provider: &str, field: &str) -> Option<String> {
    let env = default_env_mapping(provider).ok()?;
    let env_name = env.get(field)?;
    env::var(env_name).ok().filter(|value| !value.is_empty())
}

fn default_model_for_provider(provider: &str) -> String {
    if provider == "openai" {
        DEFAULT_MODEL.to_string()
    } else {
        provider_default_model(provider)
            .ok()
            .flatten()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string())
    }
}

fn workspace_from_args(args: &[String]) -> PathBuf {
    value_for(args, &["--workspace", "--dir"])
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn session_root_from_args(args: &[String]) -> PathBuf {
    value_for(args, &["--session-root"])
        .or_else(|| env::var("OPENAGENT_SESSION_ROOT").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_from_args(args).join(".openagent/sessions"))
}

fn auth_file_from_args(args: &[String]) -> PathBuf {
    value_for(args, &["--auth-file"])
        .or_else(|| env::var("OPENAGENT_AUTH_FILE").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config/openagent/auth.json"))
}

fn mcp_config_path(args: &[String]) -> PathBuf {
    value_for(args, &["--config"])
        .or_else(|| env::var("OPENAGENT_MCP_CONFIG").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_from_args(args).join(".openagent/mcp.json"))
}

fn home_dir() -> PathBuf {
    env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn read_json_file(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn write_json_file(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let raw = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, format!("{raw}\n")).map_err(|error| error.to_string())
}

fn ensure_object_field<'a>(value: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    let object = value.as_object_mut().expect("object ensured");
    object.entry(key.to_string()).or_insert_with(|| json!({}));
    object
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object field ensured")
}

#[cfg(unix)]
fn chmod_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        let _ = fs::set_permissions(path, permissions);
    }
}

#[cfg(not(unix))]
fn chmod_private(_path: &Path) {}

fn attached_files(workspace: &Path, files: &[String]) -> Result<Vec<(String, String)>, String> {
    files
        .iter()
        .map(|file| {
            let path = workspace.join(file);
            fs::read_to_string(&path)
                .map(|content| (path.to_string_lossy().to_string(), content))
                .map_err(|error| {
                    format!("failed to read attached file {}: {error}", path.display())
                })
        })
        .collect()
}

fn build_prompt_with_files(message: &str, files: &[(String, String)]) -> String {
    let mut prompt = message.trim().to_string();
    for (path, content) in files {
        prompt.push_str(&format!(
            "\n\nAttached file: {path}\n\n```text\n{content}\n```"
        ));
    }
    prompt
}

fn parse_headers(headers: &[String]) -> Map<String, Value> {
    headers
        .iter()
        .filter_map(|header| {
            header
                .split_once('=')
                .map(|(key, value)| (key.trim().to_string(), json!(value.trim())))
        })
        .filter(|(key, _)| !key.is_empty())
        .collect()
}

fn mcp_public_servers(config: &Value) -> Vec<Value> {
    config
        .get("mcp")
        .and_then(Value::as_object)
        .map(|servers| {
            servers
                .iter()
                .map(|(name, server)| mcp_public_server(name, server))
                .collect()
        })
        .unwrap_or_default()
}

fn mcp_public_server(name: &str, server: &Value) -> Value {
    let transport = server
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    let headers = server
        .get("headers")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .keys()
                .map(|key| (key.clone(), json!("[redacted]")))
                .collect::<Map<_, _>>()
        })
        .unwrap_or_default();
    json!({
        "name": name,
        "url": redact_url(server.get("url").and_then(Value::as_str).unwrap_or("")),
        "enabled": server.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        "transport": transport,
        "configured_transport": transport,
        "selected_transport": null,
        "timeout_ms": server.get("timeout_ms").and_then(Value::as_u64).unwrap_or(30_000),
        "header_names": headers.keys().cloned().collect::<Vec<_>>(),
        "headers": headers,
    })
}

fn redact_url(url: &str) -> String {
    let mut redacted = url.to_string();
    if let Some((scheme, rest)) = redacted.split_once("://")
        && let Some((_credentials, host_rest)) = rest.split_once('@')
    {
        redacted = format!("{scheme}://[redacted]@{host_rest}");
    }
    for marker in ["token=", "api_key=", "apikey=", "secret="] {
        if let Some(index) = redacted.to_ascii_lowercase().find(marker) {
            let start = index + marker.len();
            let end = redacted[start..]
                .find('&')
                .map(|offset| start + offset)
                .unwrap_or(redacted.len());
            redacted.replace_range(start..end, "[redacted]");
        }
    }
    redacted
}

fn public_auth_record(provider: &str, value: &Value, source: &str) -> Value {
    let api_key = value.get("api_key").and_then(Value::as_str).unwrap_or("");
    let present_env = env::vars().map(|(key, _)| key).collect::<BTreeSet<_>>();
    let auth_methods = provider_auth_methods(provider, &present_env).unwrap_or_default();
    json!({
        "provider": provider,
        "type": value.get("type").and_then(Value::as_str).unwrap_or("api"),
        "source": source,
        "api_key": mask_secret(api_key),
        "has_api_key": !api_key.is_empty(),
        "base_url": value.get("base_url").cloned().unwrap_or(Value::Null),
        "model": value.get("model").cloned().unwrap_or(Value::Null),
        "wire_api": value.get("wire_api").cloned().unwrap_or(Value::Null),
        "env": default_env_mapping(provider).unwrap_or_default(),
        "auth_methods": auth_methods,
        "methods": ["api_key"],
        "updated_at_ms": value.get("updated_at_ms").cloned().unwrap_or(Value::Null),
    })
}

fn valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id
            .chars()
            .all(|item| item.is_ascii_alphanumeric() || matches!(item, '_' | '-'))
}

fn sanitize_session_state(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.insert("workspace".to_string(), json!("[redacted]"));
        if let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) {
            for message in messages {
                if let Some(message_object) = message.as_object_mut() {
                    message_object.insert("content".to_string(), json!("[redacted]"));
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct CustomCommand {
    name: String,
    path: PathBuf,
    description: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    template: String,
}

impl CustomCommand {
    fn public(&self, include_template: bool) -> Value {
        let mut object = Map::from_iter([
            ("name".to_string(), json!(self.name)),
            ("path".to_string(), json!(self.path.to_string_lossy())),
            ("scope".to_string(), json!("project")),
            (
                "description".to_string(),
                self.description.clone().map_or(Value::Null, Value::String),
            ),
            (
                "agent".to_string(),
                self.agent.clone().map_or(Value::Null, Value::String),
            ),
            (
                "model".to_string(),
                self.model.clone().map_or(Value::Null, Value::String),
            ),
        ]);
        if include_template {
            object.insert("template".to_string(), json!(self.template));
        }
        Value::Object(object)
    }
}

fn discover_custom_commands(args: &[String]) -> Vec<CustomCommand> {
    let workspace = workspace_from_args(args);
    let mut dirs = vec![workspace.join(".openagent/commands")];
    dirs.extend(
        values_for(args, &["--command-dir"])
            .into_iter()
            .map(PathBuf::from),
    );
    let mut commands = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|item| item.to_str()) != Some("md") {
                continue;
            }
            if let Ok(raw) = fs::read_to_string(&path) {
                let name = path
                    .file_stem()
                    .and_then(|item| item.to_str())
                    .unwrap_or_default()
                    .to_string();
                if !name.is_empty() {
                    commands.push(parse_custom_command(name, path, &raw));
                }
            }
        }
    }
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    commands
}

fn parse_custom_command(name: String, path: PathBuf, raw: &str) -> CustomCommand {
    let mut description = None;
    let mut agent = None;
    let mut model = None;
    let mut template = raw.to_string();
    if let Some(rest) = raw.strip_prefix("---")
        && let Some((frontmatter, body)) = rest.split_once("---")
    {
        template = body.trim_start_matches('\n').to_string();
        for line in frontmatter.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let value = value.trim().trim_matches('"').to_string();
                match key.trim() {
                    "description" => description = Some(value),
                    "agent" => agent = Some(value),
                    "model" => model = Some(value),
                    _ => {}
                }
            }
        }
    }
    CustomCommand {
        name,
        path,
        description,
        agent,
        model,
        template,
    }
}

fn render_custom_template(template: &str, args: &[String], workspace: &Path) -> String {
    let arguments = args.join(" ");
    let first = args.first().cloned().unwrap_or_default();
    let mut rendered = template
        .replace("$ARGUMENTS", &arguments)
        .replace("$1", &first);
    let mut attachments = Vec::new();
    for word in rendered.split_whitespace() {
        if let Some(path) = word.strip_prefix('@') {
            let clean =
                path.trim_matches(|item: char| matches!(item, ',' | '.' | ';' | ':' | ')' | ']'));
            let target = workspace.join(clean);
            if let Ok(content) = fs::read_to_string(&target) {
                attachments.push(format!(
                    "Attached file: {}\n\n```text\n{}\n```",
                    target.to_string_lossy(),
                    content
                ));
            }
        }
    }
    if !attachments.is_empty() {
        rendered.push_str("\n\n");
        rendered.push_str(&attachments.join("\n\n"));
    }
    rendered
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub const fn command_name() -> &'static str {
    "openagent"
}

#[must_use]
pub fn core_crate_name() -> &'static str {
    openagent_core::crate_name()
}

#[must_use]
pub fn parse_cli_args(argv: &[&str]) -> Value {
    if argv.is_empty() {
        return json!({
            "command": null,
            "base_url": null,
            "model": null,
            "wire_api": null,
            "max_steps": null,
            "workspace": null,
            "skip_doctor": false,
        });
    }
    match argv[0] {
        "doctor" => json!({
            "command": "doctor",
            "format": value_after(argv, "--format").unwrap_or("text"),
            "base_url": value_after(argv, "--base-url"),
            "model": value_after(argv, "--model"),
        }),
        "run" => json!({
            "command": "run",
            "workspace": value_after(argv, "--workspace").or_else(|| value_after(argv, "--dir")),
            "skip_doctor": argv.contains(&"--skip-doctor"),
            "format": value_after(argv, "--format").unwrap_or("text"),
            "message": positional_after_options(argv, &["run"]),
        }),
        "mcp" if argv.get(1) == Some(&"add") => json!({
            "command": "mcp",
            "mcp_command": "add",
            "name": argv.get(2).copied().unwrap_or_default(),
            "url": value_after(argv, "--url").unwrap_or_default(),
            "transport": value_after(argv, "--transport").unwrap_or("auto"),
            "timeout_ms": value_after(argv, "--timeout-ms").and_then(|item| item.parse::<u64>().ok()).unwrap_or(30_000),
            "format": value_after(argv, "--format").unwrap_or("table"),
        }),
        command => json!({"command": command}),
    }
}

#[must_use]
pub fn model_env_fixture() -> Value {
    json!({
        "default": {
            "OPENAI_BASE_URL": DEFAULT_BASE_URL,
            "OPENAI_MODEL": DEFAULT_MODEL,
            "OPENAI_WIRE_API": DEFAULT_WIRE_API,
            "OPENAGENT_APP_MAX_STEPS": DEFAULT_MAX_STEPS,
        },
        "override": {
            "OPENAI_BASE_URL": "http://127.0.0.1:9999",
            "OPENAI_MODEL": "gpt-test",
            "OPENAI_WIRE_API": "chat",
            "OPENAGENT_APP_MAX_STEPS": "8",
        },
    })
}

#[must_use]
pub fn doctor_text_ok_result() -> CliRunResult {
    CliRunResult {
        exit_code: 0,
        stdout: [
            "OpenAgent doctor",
            "- provider: openai (OpenAI)",
            "- OPENAI_BASE_URL: http://gateway.test",
            "- OPENAI_MODEL: gpt-test",
            "- OPENAI_WIRE_API: chat",
            "- OPENAI_API_KEY: missing",
            "- model endpoint: ok (http://gateway.test/v1/models)",
            "",
        ]
        .join("\n"),
        stderr: String::new(),
    }
}

#[must_use]
pub fn doctor_json_failed_payload() -> Value {
    json!({
        "provider": "openai",
        "provider_label": "OpenAI",
        "base_url": "http://gateway.test",
        "model": "gpt-test",
        "wire_api": "responses",
        "api_key_env": "OPENAI_API_KEY",
        "api_key_set": true,
        "native": false,
        "healthy": false,
        "dependency_checked": false,
        "dependency_ok": true,
        "dependency_message": null,
        "model_endpoint_checked": true,
        "model_endpoint_ok": false,
        "model_endpoint_message": "connection refused",
    })
}

#[must_use]
pub fn doctor_json_failed_result() -> CliRunResult {
    let payload = doctor_json_failed_payload();
    CliRunResult {
        exit_code: 2,
        stdout: format!("{}\n", python_json_dumps(&payload)),
        stderr: String::new(),
    }
}

#[must_use]
pub fn doctor_anthropic_payload() -> Value {
    json!({
        "provider": "anthropic",
        "provider_label": "Anthropic",
        "base_url": null,
        "model": "claude-test",
        "wire_api": "messages",
        "api_key_env": "ANTHROPIC_API_KEY",
        "api_key_set": true,
        "native": true,
        "healthy": true,
        "dependency_checked": true,
        "dependency_ok": true,
        "dependency_message": "optional dependency 'anthropic' is installed",
        "model_endpoint_checked": false,
        "model_endpoint_ok": true,
        "model_endpoint_message": "skipped OpenAI-compatible /models probe for native provider",
    })
}

#[must_use]
pub fn auth_login_payload() -> Value {
    json!({
        "status": "logged_in",
        "provider": "groq",
        "auth_file": format!("{GOAL10_ROOT}/auth.json"),
        "record": public_provider_record(
            "groq",
            "groq-secret",
            "https://api.groq.example/v1",
            "llama-fixture",
            Value::Null,
            "auth_file",
            Some(1_781_842_000_123u64),
        ),
    })
}

#[must_use]
pub fn auth_list_payload() -> Value {
    json!({
        "auth_file": format!("{GOAL10_ROOT}/auth.json"),
        "providers": [
            public_provider_record(
                "groq",
                "groq-secret",
                "https://api.groq.example/v1",
                "llama-fixture",
                Value::Null,
                "auth_file",
                Some(1_781_842_000_123u64),
            )
        ],
    })
}

#[must_use]
pub fn auth_methods_payload() -> Value {
    let methods =
        provider_auth_methods("openrouter", &BTreeSet::new()).expect("openrouter methods build");
    json!({"provider": "openrouter", "methods": methods})
}

#[must_use]
pub fn custom_command_list_payload() -> Value {
    json!({
        "commands": [custom_command_record(false)],
    })
}

#[must_use]
pub fn custom_command_show_payload() -> Value {
    custom_command_record(true)
}

#[must_use]
pub fn rendered_custom_command_prompt() -> String {
    format!(
        "Review notes.txt with all args: notes.txt carefully.\n\n\
         Attached file: {GOAL10_WORKSPACE}/notes.txt\n\n\
         ```text\nAlpha note\nBeta note\n\n```"
    )
}

#[must_use]
pub fn custom_command_render_text_result() -> CliRunResult {
    CliRunResult {
        exit_code: 0,
        stdout: format!("{}\n", rendered_custom_command_prompt()),
        stderr: String::new(),
    }
}

#[must_use]
pub fn custom_command_render_json_payload() -> Value {
    json!({
        "command": custom_command_record(false),
        "prompt": rendered_custom_command_prompt(),
    })
}

#[must_use]
pub fn config_init_payload() -> Value {
    json!({
        "created": true,
        "path": format!("{GOAL10_WORKSPACE}/.openagent/openagent.env"),
        "workspace": GOAL10_WORKSPACE,
        "api_key_written": true,
        "server_token_written": false,
        "mode": "0o600",
        "next": ["openagent doctor", "openagent"],
    })
}

#[must_use]
pub fn config_show_payload() -> Value {
    json!({
        "workspace": GOAL10_WORKSPACE,
        "env_file": format!("{GOAL10_WORKSPACE}/.openagent/openagent.env"),
        "auth_file": format!("{GOAL10_ROOT}/auth.json"),
        "session_root": format!("{GOAL10_WORKSPACE}/.openagent/sessions"),
        "openai": {
            "base_url": "http://config.test/v1",
            "model": "gpt-config",
            "wire_api": "responses",
            "api_key": "set",
            "max_steps": "12",
        },
        "app_bridge": {
            "server_url": DEFAULT_SERVER_URL,
            "server_token": "set",
            "server_token_env": DEFAULT_SERVER_TOKEN_ENV,
        },
    })
}

#[must_use]
pub fn mcp_add_payload() -> Value {
    json!({
        "config_path": format!("{GOAL10_ROOT}/mcp.json"),
        "server": {
            "name": "demo",
            "url": "https://[redacted]@example.com/mcp?token=[redacted]&safe=1",
            "transport": "http",
            "enabled": true,
            "timeout_ms": 45_000,
            "header_names": ["Authorization", "X-Team"],
            "headers": {"Authorization": "[redacted]", "X-Team": "[redacted]"},
        },
        "updated": true,
    })
}

#[must_use]
pub fn mcp_list_table_result() -> CliRunResult {
    CliRunResult {
        exit_code: 0,
        stdout: [
            "name  enabled  transport  timeout_ms  headers               url",
            "----  -------  ---------  ----------  --------------------  ----------------------------------------------------------",
            "demo  True     http       45000       Authorization,X-Team  https://[redacted]@example.com/mcp?token=[redacted]&safe=1",
            "",
        ]
        .join("\n"),
        stderr: String::new(),
    }
}

#[must_use]
pub fn mcp_doctor_payload() -> Value {
    json!({
        "config_path": format!("{GOAL10_ROOT}/mcp.json"),
        "configured": true,
        "enabled": true,
        "server_count": 1,
        "ok": true,
        "refresh_error": null,
        "servers": [{
            "name": "demo",
            "url": "https://[redacted]@example.com/mcp?token=[redacted]&safe=1",
            "enabled": true,
            "configured_transport": "http",
            "selected_transport": null,
            "status": "idle",
            "tool_count": 0,
            "last_error": null,
            "last_refreshed_at": null,
            "tools": [],
            "ok": true,
        }],
    })
}

#[must_use]
pub fn cli_commands_fixture() -> Value {
    json!({
        "schema_version": 1,
        "parser": {
            "default": {
                "argv": [],
                "namespace": parse_cli_args(&[]),
            },
            "doctor_json": {
                "argv": ["doctor", "--format", "json"],
                "namespace": parse_cli_args(&["doctor", "--format", "json"]),
            },
            "run_json": {
                "argv": ["run", "--workspace", GOAL10_WORKSPACE, "--skip-doctor", "--format", "json", "hello", "world"],
                "namespace": parse_cli_args(&["run", "--workspace", GOAL10_WORKSPACE, "--skip-doctor", "--format", "json", "hello", "world"]),
            },
            "mcp_add": {
                "argv": ["mcp", "add", "demo", "--config", format!("{GOAL10_ROOT}/mcp.json"), "--url", "https://example.com/mcp"],
                "namespace": parse_cli_args(&["mcp", "add", "demo", "--config", &format!("{GOAL10_ROOT}/mcp.json"), "--url", "https://example.com/mcp"]),
            },
        },
        "model_env": model_env_fixture(),
        "doctor": {
            "text_ok": run_result_json_without_stderr(&doctor_text_ok_result(), None),
            "json_failed": run_result_json_without_stderr(&doctor_json_failed_result(), Some(doctor_json_failed_payload())),
            "anthropic_json": {
                "exit_code": 0,
                "json": doctor_anthropic_payload(),
                "stdout": format!("{}\n", python_json_dumps(&doctor_anthropic_payload())),
                "openai_probe_called": false,
            },
        },
        "auth": {
            "login": run_result_json(&CliRunResult::ok_json(&auth_login_payload()), Some(auth_login_payload())),
            "list": run_result_json(&CliRunResult::ok_json(&auth_list_payload()), Some(auth_list_payload())),
            "methods": run_result_json(&CliRunResult::ok_json(&auth_methods_payload()), Some(auth_methods_payload())),
        },
        "custom_commands": {
            "list": run_result_json(&CliRunResult::ok_json(&custom_command_list_payload()), Some(custom_command_list_payload())),
            "show": run_result_json(&CliRunResult::ok_json(&custom_command_show_payload()), Some(custom_command_show_payload())),
            "render_text": run_result_json(&custom_command_render_text_result(), None),
            "render_json": run_result_json(&CliRunResult::ok_json(&custom_command_render_json_payload()), Some(custom_command_render_json_payload())),
        },
        "config": {
            "init": run_result_json(&CliRunResult::ok_json(&config_init_payload()), Some(config_init_payload())),
            "show": run_result_json(&CliRunResult::ok_json(&config_show_payload()), Some(config_show_payload())),
        },
        "mcp_cli": {
            "add": run_result_json(&CliRunResult::ok_json(&mcp_add_payload()), Some(mcp_add_payload())),
            "list_table": run_result_json(&mcp_list_table_result(), None),
            "doctor": run_result_json(&CliRunResult::ok_json(&mcp_doctor_payload()), Some(mcp_doctor_payload())),
        },
    })
}

#[must_use]
pub fn run_cli_command(argv: &[String]) -> CliRunResult {
    if argv.is_empty() {
        return CliRunResult {
            exit_code: 0,
            stdout: format!("{}\n", command_name()),
            stderr: String::new(),
        };
    }
    if is_help_flag(&argv[0]) || argv[0] == "help" {
        return ok_text(root_help());
    }
    match argv[0].as_str() {
        "tui" => tui_command(&argv[1..]),
        "serve" => http_runtime_command(&argv[1..], false, serve_help()),
        "web" => http_runtime_command(&argv[1..], true, web_help()),
        "attach" => attach_command(&argv[1..]),
        "run" => run_prompt_command(&argv[1..]),
        "client" => client_command(&argv[1..]),
        "session" => session_command(&argv[1..]),
        "models" => models_command(&argv[1..]),
        "stats" => stats_command(&argv[1..]),
        "command" => custom_command(&argv[1..]),
        "config" => config_command(&argv[1..]),
        "auth" | "providers" => auth_command(argv[0].as_str(), &argv[1..]),
        "mcp" => mcp_command(&argv[1..]),
        "approval" => approval_command(&argv[1..]),
        "question" => question_command(&argv[1..]),
        "doctor" => {
            if argv.iter().any(|arg| is_help_flag(arg)) {
                return ok_text(doctor_help());
            }
            let format = argv
                .windows(2)
                .find_map(|items| (items[0] == "--format").then_some(items[1].as_str()))
                .unwrap_or("text");
            let provider = env::var("OPENAGENT_PROVIDER")
                .or_else(|_| env::var("OPENAGENT_ACTIVE_PROVIDER"))
                .unwrap_or_else(|_| "openai".to_string());
            let payload = doctor_payload_from_args(&provider, &argv[1..]);
            let healthy = payload
                .get("healthy")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if format == "json" {
                CliRunResult {
                    exit_code: if healthy { 0 } else { 2 },
                    stdout: format!("{}\n", python_json_dumps(&payload)),
                    stderr: String::new(),
                }
            } else {
                CliRunResult {
                    exit_code: if healthy { 0 } else { 2 },
                    stdout: doctor_text_from_payload(&payload),
                    stderr: String::new(),
                }
            }
        }
        "agent" => agent_command(&argv[1..]),
        "plugin" | "plug" => plugin_command(&argv[1..]),
        "github" => github_command(&argv[1..]),
        "pr" => pr_command(&argv[1..]),
        "debug" => debug_command(&argv[1..]),
        "db" => db_command(&argv[1..]),
        "upgrade" => lifecycle_command("upgrade", &argv[1..]),
        "uninstall" => lifecycle_command("uninstall", &argv[1..]),
        "acp" => acp_command(&argv[1..]),
        "import" => session_import(&argv[1..]),
        "export" => session_export(&argv[1..]),
        "generate" => generate_command(&argv[1..]),
        "console" => console_command(&argv[1..]),
        _ => CliRunResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: format!("unsupported Rust CLI command: {}\n", argv[0]),
        },
    }
}

#[must_use]
pub fn run_cli_command_streaming(argv: &[String]) -> CliRunResult {
    if argv.first().map(String::as_str) == Some("run")
        && value_for(&argv[1..], &["--format"]).as_deref() == Some("json")
    {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        let mut emit = |event: &Value| {
            let _ = writeln!(handle, "{}", python_json_dumps(event));
            let _ = handle.flush();
        };
        return run_prompt_command_with_events(&argv[1..], Some(&mut emit));
    }
    run_cli_command(argv)
}

#[must_use]
pub fn python_json_dumps(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => {
            if *flag {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(number) => number.to_string(),
        Value::String(text) => serde_json::to_string(text).expect("string serializes"),
        Value::Array(items) => items
            .iter()
            .map(python_json_dumps)
            .collect::<Vec<_>>()
            .join(", ")
            .pipe(|inner| format!("[{inner}]")),
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            keys.into_iter()
                .map(|key| {
                    format!(
                        "{}: {}",
                        serde_json::to_string(key).expect("key serializes"),
                        python_json_dumps(&object[key])
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
                .pipe(|inner| format!("{{{inner}}}"))
        }
    }
}

fn run_result_json(result: &CliRunResult, json_value: Option<Value>) -> Value {
    let mut object = Map::from_iter([
        ("exit_code".to_string(), json!(result.exit_code)),
        ("stdout".to_string(), json!(result.stdout)),
        ("stderr".to_string(), json!(result.stderr)),
    ]);
    if let Some(json_value) = json_value {
        object.insert("json".to_string(), json_value);
    }
    Value::Object(object)
}

fn run_result_json_without_stderr(result: &CliRunResult, json_value: Option<Value>) -> Value {
    let mut object = Map::from_iter([
        ("exit_code".to_string(), json!(result.exit_code)),
        ("stdout".to_string(), json!(result.stdout)),
    ]);
    if let Some(json_value) = json_value {
        object.insert("json".to_string(), json_value);
    }
    Value::Object(object)
}

fn public_provider_record(
    provider: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
    wire_api: Value,
    source: &str,
    updated_at_ms: Option<u64>,
) -> Value {
    let env = default_env_mapping(provider).expect("provider env mapping exists");
    let auth_methods = provider_auth_methods(provider, &BTreeSet::new())
        .expect("provider auth methods build")
        .into_iter()
        .map(|mut method| {
            if let Some(object) = method.as_object_mut() {
                let keep = ["id", "type", "env_api_key", "implemented", "status"];
                let api_key_env = object
                    .get("env")
                    .and_then(Value::as_object)
                    .and_then(|env| env.get("api_key"))
                    .cloned()
                    .unwrap_or(Value::Null);
                object.retain(|key, _| keep.contains(&key.as_str()));
                object.insert("env_api_key".to_string(), api_key_env);
            }
            method
        })
        .collect::<Vec<_>>();
    let env_status = env
        .iter()
        .map(|(field, name)| (field.clone(), json!({"name": name, "status": "missing"})))
        .collect::<Map<_, _>>();
    json!({
        "provider": provider,
        "type": "api",
        "source": source,
        "api_key": mask_secret(api_key),
        "has_api_key": true,
        "base_url": base_url,
        "model": model,
        "wire_api": wire_api,
        "env": env,
        "env_status": env_status,
        "auth_methods": auth_methods,
        "methods": ["api_key"],
        "updated_at_ms": updated_at_ms,
    })
}

fn custom_command_record(include_template: bool) -> Value {
    let mut object = Map::from_iter([
        ("name".to_string(), json!("review")),
        (
            "path".to_string(),
            json!(format!("{GOAL10_WORKSPACE}/.openagent/commands/review.md")),
        ),
        ("scope".to_string(), json!("project")),
        ("description".to_string(), json!("Review a target file.")),
        ("agent".to_string(), json!("reviewer")),
        ("model".to_string(), json!("gpt-command")),
    ]);
    if include_template {
        object.insert(
            "template".to_string(),
            json!("Review $1 with all args: $ARGUMENTS.\n\n@notes.txt"),
        );
    }
    Value::Object(object)
}

fn doctor_payload_from_args(provider: &str, args: &[String]) -> Value {
    let mut payload = doctor_payload_from_env(provider);
    let Ok(normalized) = normalize_provider(Some(provider)) else {
        return payload;
    };
    let Some(object) = payload.as_object_mut() else {
        return payload;
    };
    if normalized == "anthropic" {
        if let Some(api_key) = value_for(args, &["--api-key"])
            && !api_key.is_empty()
        {
            object.insert("api_key_set".to_string(), json!(true));
            object.insert(
                "healthy".to_string(),
                json!(bool_field(object, "dependency_ok")),
            );
            object.insert(
                "model_endpoint_ok".to_string(),
                json!(bool_field(object, "dependency_ok")),
            );
        }
        if let Some(base_url) = value_for(args, &["--base-url"]) {
            object.insert("base_url".to_string(), json!(base_url));
        }
        if let Some(model) = value_for(args, &["--model", "-m"]) {
            object.insert("model".to_string(), json!(model));
        }
        if let Some(wire_api) = value_for(args, &["--wire-api"]) {
            object.insert("wire_api".to_string(), json!(wire_api));
        }
        return payload;
    }

    if let Some(api_key) = value_for(args, &["--api-key"])
        && !api_key.is_empty()
    {
        object.insert("api_key_set".to_string(), json!(true));
    }
    if let Some(base_url) = value_for(args, &["--base-url"]) {
        object.insert("base_url".to_string(), json!(base_url));
    }
    if let Some(model) = value_for(args, &["--model", "-m"]) {
        object.insert("model".to_string(), json!(model));
    }
    if let Some(wire_api) = value_for(args, &["--wire-api"]) {
        object.insert("wire_api".to_string(), json!(wire_api));
    }
    payload
}

fn doctor_payload_from_env(provider: &str) -> Value {
    if provider == "anthropic" {
        return json!({
            "provider": "anthropic",
            "provider_label": "Anthropic",
            "base_url": env::var("ANTHROPIC_BASE_URL").ok(),
            "model": env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string()),
            "wire_api": "messages",
            "api_key_env": "ANTHROPIC_API_KEY",
            "api_key_set": env::var("ANTHROPIC_API_KEY").is_ok_and(|value| !value.is_empty()),
            "native": true,
            "healthy": env::var("ANTHROPIC_API_KEY").is_ok_and(|value| !value.is_empty()),
            "dependency_checked": true,
            "dependency_ok": true,
            "dependency_message": "optional dependency 'anthropic' is installed",
            "model_endpoint_checked": false,
            "model_endpoint_ok": env::var("ANTHROPIC_API_KEY").is_ok_and(|value| !value.is_empty()),
            "model_endpoint_message": "skipped OpenAI-compatible /models probe for native provider",
        });
    }
    let endpoint_ok = env::var("OPENAGENT_DOCTOR_MODEL_ENDPOINT_OK")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes"));
    json!({
        "provider": "openai",
        "provider_label": "OpenAI",
        "base_url": env::var("OPENAI_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()),
        "model": env::var("OPENAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
        "wire_api": env::var("OPENAI_WIRE_API").unwrap_or_else(|_| DEFAULT_WIRE_API.to_string()),
        "api_key_env": "OPENAI_API_KEY",
        "api_key_set": env::var("OPENAI_API_KEY").is_ok_and(|value| !value.is_empty()),
        "native": false,
        "healthy": endpoint_ok,
        "dependency_checked": false,
        "dependency_ok": true,
        "dependency_message": null,
        "model_endpoint_checked": true,
        "model_endpoint_ok": endpoint_ok,
        "model_endpoint_message": env::var("OPENAGENT_DOCTOR_MODEL_ENDPOINT_MESSAGE").unwrap_or_else(|_| "not checked by Rust CLI smoke".to_string()),
    })
}

fn doctor_text_from_payload(payload: &Value) -> String {
    let object = payload.as_object().expect("doctor payload object");
    let healthy = bool_field(object, "healthy");
    let api_key = if bool_field(object, "api_key_set") {
        "set"
    } else {
        "missing"
    };
    if object
        .get("native")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut text = render_key_values(
            "OpenAgent Doctor",
            &[
                (
                    "Status",
                    if healthy { "ok" } else { "needs attention" }.to_string(),
                ),
                (
                    "Provider",
                    format!(
                        "{} ({})",
                        string_field(object, "provider_label"),
                        string_field(object, "provider")
                    ),
                ),
                ("Model", string_field(object, "model")),
                (
                    "API Key",
                    format!("{api_key} ({})", string_field(object, "api_key_env")),
                ),
                ("Base URL", string_field(object, "base_url")),
            ],
        );
        text.push_str("\n\n");
        text.push_str(&render_table(
            &["Check", "Status", "Detail"],
            &[
                vec![
                    "Dependency".to_string(),
                    if bool_field(object, "dependency_ok") {
                        "ok".to_string()
                    } else {
                        "missing".to_string()
                    },
                    string_field(object, "dependency_message"),
                ],
                vec![
                    "Model Endpoint".to_string(),
                    "skipped".to_string(),
                    string_field(object, "model_endpoint_message"),
                ],
            ],
        ));
        text.push('\n');
        return text;
    }
    let mut text = render_key_values(
        "OpenAgent Doctor",
        &[
            (
                "Status",
                if healthy { "ok" } else { "needs attention" }.to_string(),
            ),
            (
                "Provider",
                format!(
                    "{} ({})",
                    string_field(object, "provider_label"),
                    string_field(object, "provider")
                ),
            ),
            ("Model", string_field(object, "model")),
            ("Wire API", string_field(object, "wire_api")),
            (
                "API Key",
                format!("{api_key} ({})", string_field(object, "api_key_env")),
            ),
            ("Base URL", string_field(object, "base_url")),
        ],
    );
    text.push_str("\n\n");
    text.push_str(&render_table(
        &["Check", "Status", "Detail"],
        &[vec![
            "Model Endpoint".to_string(),
            if bool_field(object, "model_endpoint_ok") {
                "ok".to_string()
            } else {
                "failed".to_string()
            },
            string_field(object, "model_endpoint_message"),
        ]],
    ));
    text.push('\n');
    text
}

fn string_field(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn bool_field(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    if value.len() <= 8 {
        return "*".repeat(value.len());
    }
    format!(
        "{}{}{}",
        &value[..4],
        "*".repeat((value.len() - 8).max(4)),
        &value[value.len() - 4..]
    )
}

fn looks_secret(value: &str) -> bool {
    value.len() >= 12
        || value.starts_with("sk-")
        || value.contains("token")
        || value.contains("secret")
        || value.contains("Bearer ")
}

fn sanitize_identifier(value: &str) -> String {
    let mut output = value
        .trim()
        .chars()
        .map(|item| {
            if item.is_ascii_alphanumeric() || matches!(item, '-' | '_') {
                item.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    while output.contains("--") {
        output = output.replace("--", "-");
    }
    if output.is_empty() {
        "item".to_string()
    } else {
        output
    }
}

fn copy_cli_options(args: &[String], names: &[&str], output: &mut Vec<String>) {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let name = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        if !names.contains(&name) {
            index += 1;
            continue;
        }
        output.push(arg.clone());
        if arg.contains('=') || matches!(name, "--skip-doctor" | "--stream") {
            index += 1;
            continue;
        }
        if let Some(value) = args.get(index + 1)
            && !value.starts_with('-')
        {
            output.push(value.clone());
            index += 2;
            continue;
        }
        index += 1;
    }
}

fn value_after<'a>(argv: &'a [&'a str], flag: &str) -> Option<&'a str> {
    argv.windows(2)
        .find_map(|items| (items[0] == flag).then_some(items[1]))
}

fn positional_after_options(argv: &[&str], skip: &[&str]) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < argv.len() {
        let item = argv[index];
        if skip.contains(&item) {
            index += 1;
            continue;
        }
        if item.starts_with("--") {
            let takes_value = matches!(
                item,
                "--workspace" | "--dir" | "--format" | "--session" | "--session-root" | "--command"
            );
            index += if takes_value { 2 } else { 1 };
            continue;
        }
        values.push(item.to_string());
        index += 1;
    }
    values
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_command_boundary() {
        assert_eq!(crate_name(), "openagent-cli");
        assert_eq!(command_name(), "openagent");
        assert_eq!(core_crate_name(), "openagent-core");
    }
}
