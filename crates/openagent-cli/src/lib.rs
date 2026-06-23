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

fn root_help() -> &'static str {
    "OpenAgent Rust CLI\n\n\
     Usage: openagent <command> [options]\n\n\
     Commands:\n\
       tui          start the terminal UI\n\
       run          run a prompt without launching the TUI\n\
       serve        start the local App Bridge HTTP server\n\
       web          start the browser console\n\
       client       send a prompt to a running App Bridge server\n\
       attach       attach the terminal UI to a running App Bridge server\n\
       session      manage stored sessions\n\
       models       list configured model metadata\n\
       stats        show local session usage statistics\n\
       command      manage custom prompt commands\n\
       config       inspect and initialize local CLI configuration\n\
       auth         manage provider credentials\n\
       providers    provider credential alias for auth\n\
       mcp          manage remote MCP servers\n\
       approval     inspect or answer queued approval requests\n\
       question     inspect or answer queued question requests\n\
       doctor       check local model gateway configuration\n\n\
     Additional OpenCode parity commands: agent, plugin, github, pr, debug, db, upgrade, uninstall, acp, import, export, generate, console"
}

fn run_help() -> &'static str {
    "Usage: openagent run [message..] [options]\n\n\
     Options:\n\
       --command <name>                 custom command name\n\
       -c, --continue                   continue the last session\n\
       -s, --session <id>               continue a session\n\
       --fork                           fork before continuing (OpenCode parity flag)\n\
       --share                          share the session (OpenCode parity flag)\n\
       -m, --model <provider/model>     model to use\n\
       --agent <name>                   agent profile to use\n\
       -f, --file <path>                attach a file; repeatable\n\
       --format <text|json|default>     output format\n\
       --title <title>                  session title\n\
       --attach <url>                   attach to a running server\n\
       --server-token <token>           bearer token for --attach\n\
       -u, --username <name>            basic auth username for --attach\n\
       -p, --password <password>        basic auth password for --attach\n\
       --mcp-config <path-or-json>      enable remote MCP tools for this run\n\
       --answer <text>                  answer one queued question tool prompt; repeatable\n\
       --dir, --workspace <path>        workspace path\n\
       --session-root <path>            session store root\n\
       --variant <name>                 provider-specific variant\n\
       --thinking                       show thinking blocks\n\
       --interactive, -i                run direct interactive mode\n\
       --permission <ruleset>           FULL, READONLY, PLAN_ONLY, NONE; default PLAN_ONLY\n\
       --dangerously-skip-permissions   auto-approve permissions that are not denied\n\
       --stream                         use native provider SSE streaming when available\n\
       --skip-doctor                    skip local gateway check"
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

fn run_prompt_command(args: &[String]) -> CliRunResult {
    run_prompt_command_with_events(args, None)
}

fn run_prompt_command_with_events(
    args: &[String],
    mut event_sink: Option<&mut dyn FnMut(&Value)>,
) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(run_help());
    }
    let format = value_for(args, &["--format"]).unwrap_or_else(|| "text".to_string());
    if let Some(url) = value_for(args, &["--attach"]) {
        return run_attached_command(args, &url, event_sink);
    }
    let workspace = workspace_from_args(args);
    let agent_profile = match load_agent_profile_from_args(args, &workspace) {
        Ok(profile) => profile,
        Err(error) => return err_text(2, error),
    };
    let (provider, model_id) = provider_and_model_from_args(args, agent_profile.as_ref());
    if !has_flag(args, &["--skip-doctor"])
        && !doctor_payload_from_args(&provider, args)["healthy"]
            .as_bool()
            .unwrap_or(false)
    {
        return err_text(
            2,
            "Gateway check failed. Start your local OpenAI-compatible service, or rerun with --skip-doctor.",
        );
    }
    let permission_ruleset = match permission_ruleset_from_args(args, agent_profile.as_ref()) {
        Ok(ruleset) => ruleset,
        Err(error) => return err_text(2, error),
    };
    let skip_permissions = has_flag(args, &["--dangerously-skip-permissions"]);
    let agent_name = agent_profile
        .as_ref()
        .map(|profile| profile.id.clone())
        .or_else(|| value_for(args, &["--agent"]))
        .unwrap_or_else(|| "default".to_string());
    let max_steps = value_for(args, &["--max-steps"])
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| DEFAULT_MAX_STEPS.parse::<u64>().unwrap_or(30));
    let run_selection = match prepare_run_session(args, &workspace, &provider, &model_id) {
        Ok(selection) => selection,
        Err(error) => return err_text(1, error),
    };
    let forked = run_selection.forked;
    let mut session = run_selection.session;
    let store = run_selection.store;
    let pending_resume = pending_resume_from_session(&session);
    let (message, prompt, has_user_prompt) = match build_run_prompt(
        args,
        &workspace,
        agent_profile.as_ref(),
        pending_resume.is_some(),
    ) {
        Ok(prompt) => prompt,
        Err(error) => {
            return err_text(
                if error.contains("requires a prompt") {
                    2
                } else {
                    1
                },
                error,
            );
        }
    };
    let run_id = new_cli_id("run");
    let trace_id = new_cli_id("trace");
    session.status = SessionStatus::Running;
    session
        .metadata
        .insert("agent".to_string(), json!(agent_name.clone()));
    session
        .metadata
        .insert("provider".to_string(), json!(provider.clone()));
    session
        .metadata
        .insert("model".to_string(), json!(model_id.clone()));
    if let Some(title) = title_from_args(args, &message) {
        session.metadata.insert("title".to_string(), json!(title));
    }
    if let Some(profile) = agent_profile.as_ref() {
        session.metadata.insert(
            "agent_profile".to_string(),
            agent_profile_public_value(profile),
        );
    }
    if let Some(variant) = value_for(args, &["--variant"]) {
        session
            .metadata
            .insert("variant".to_string(), json!(variant));
    }
    session.metadata.insert(
        "thinking".to_string(),
        json!(has_flag(args, &["--thinking"])),
    );
    session.metadata.insert(
        "dangerously_skip_permissions".to_string(),
        json!(skip_permissions),
    );
    session
        .metadata
        .insert("permission".to_string(), json!(permission_ruleset.as_str()));
    if let Err(error) = store.start_run(
        &mut session,
        StartRunOptions {
            run_id: run_id.clone(),
            trace_id,
            agent_name,
            model_id: Some(model_id.clone()),
            provider_id: Some(provider.clone()),
            permission: if skip_permissions {
                format!("auto_allow:{}", permission_ruleset.as_str())
            } else {
                permission_ruleset.as_str().to_string()
            },
            max_steps,
            started_at_ms: None,
        },
    ) {
        return err_text(1, format!("failed to start session run: {error}"));
    }
    if let Err(error) =
        bind_agent_profile_system_prompt(&mut session, &store, &run_id, agent_profile.as_ref())
    {
        return err_text(1, error);
    }
    if has_user_prompt {
        let user_message = chat_message(Role::User, prompt.clone());
        let user_index = session.messages.len() as u64;
        session.add(user_message.clone());
        if let Err(error) = store.append_message(&session, &user_message, &run_id, user_index) {
            return err_text(1, format!("failed to record user message: {error}"));
        }
    }
    let loop_result = run_agent_loop(
        AgentLoopRequest {
            args,
            workspace: &workspace,
            provider: &provider,
            model_id: &model_id,
            session: &mut session,
            store: &store,
            run_id: &run_id,
            max_steps,
            prompt: &prompt,
            agent_profile: agent_profile.as_ref(),
            permission_ruleset,
            skip_permissions,
        },
        &mut event_sink,
    );
    let loop_result = match loop_result {
        Ok(result) => result,
        Err(error) => {
            session.status = if error.paused {
                SessionStatus::Paused
            } else {
                SessionStatus::Stop
            };
            let finish_reason = error.finish_reason.as_deref().unwrap_or(if error.paused {
                "paused"
            } else {
                "error"
            });
            let _ = store.finish_run(
                &session,
                &run_id,
                "failed",
                error.steps.max(1),
                Some(finish_reason),
                Some(&error.message),
            );
            if format == "json" && !error.events.is_empty() {
                return err_json_events(
                    error.events,
                    error.message,
                    if error.paused { "paused" } else { "failed" },
                    &mut event_sink,
                );
            }
            return err_text(1, error.message);
        }
    };
    let answer = loop_result.answer.clone();
    session.status = SessionStatus::Idle;
    let _ = store.record_event(
        &session.id,
        &run_id,
        "model.usage",
        SessionEventOptions {
            kind: "model".to_string(),
            attributes: BTreeMap::from([
                (
                    "input_tokens".to_string(),
                    json!(loop_result.usage.input_tokens),
                ),
                (
                    "output_tokens".to_string(),
                    json!(loop_result.usage.output_tokens),
                ),
                ("cost".to_string(), json!(loop_result.usage.cost)),
                ("source".to_string(), json!(loop_result.source.clone())),
                ("tool_calls".to_string(), json!(loop_result.tool_calls)),
            ]),
            ..SessionEventOptions::default()
        },
    );
    if let Err(error) = store.finish_run(
        &session,
        &run_id,
        "completed",
        loop_result.steps.max(1),
        Some(&loop_result.finish_reason),
        None,
    ) {
        return err_text(1, format!("failed to finish session run: {error}"));
    }
    let share = if has_flag(args, &["--share"]) {
        match share_session(&store, &session.id, false) {
            Ok(value) => Some(value),
            Err(error) => return err_text(1, error),
        }
    } else {
        None
    };
    if format == "json" {
        let mut completed = json!({
            "method": "turn/completed",
            "params": {
                "status": "completed",
                "final_answer": answer.clone(),
                "session_id": session.id.clone(),
                "run_id": run_id.clone(),
                "provider": provider.clone(),
                "source": loop_result.source,
                "forked": forked,
                "steps": loop_result.steps,
                "tool_calls": loop_result.tool_calls,
            }
        });
        if let Some(share) = share {
            completed["params"]["share"] = share;
        }
        if let Some(emit) = event_sink {
            emit(&completed);
            return CliRunResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            };
        }
        let mut events = loop_result.events;
        events.push(completed);
        return ok_text(
            events
                .iter()
                .map(python_json_dumps)
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    let mut text = answer;
    if let Some(share) = share
        && let Some(url) = share.get("url").and_then(Value::as_str)
    {
        text.push_str(&format!("\n\nShared session: {url}"));
    }
    ok_text(text)
}

fn run_attached_command(
    args: &[String],
    server_url: &str,
    event_sink: Option<&mut dyn FnMut(&Value)>,
) -> CliRunResult {
    let format = value_for(args, &["--format"]).unwrap_or_else(|| "text".to_string());
    let auth = remote_auth_from_args(args);
    let workspace = workspace_from_args(args);
    let (_, prompt, _) = match build_run_prompt(args, &workspace, None, false) {
        Ok(prompt) => prompt,
        Err(error) => {
            return err_text(
                if error.contains("requires a prompt") {
                    2
                } else {
                    1
                },
                error,
            );
        }
    };
    let session_id = match remote_select_session_with_auth(
        server_url,
        &auth,
        value_for(args, &["--session", "-s"]),
        has_flag(args, &["--continue", "-c"]),
        has_flag(args, &["--fork"]),
        &workspace,
    ) {
        Ok(session_id) => session_id,
        Err(error) => return err_text(1, error),
    };
    let payload = match remote_start_turn_with_auth(server_url, &auth, &session_id, &prompt) {
        Ok(payload) => payload,
        Err(error) => return err_text(1, error),
    };
    let events = match remote_events_for_payload(server_url, &auth, &payload) {
        Ok(events) => events,
        Err(error) => return err_text(1, error),
    };
    if format == "json" {
        if !events.is_empty() {
            if let Some(emit) = event_sink {
                for event in &events {
                    emit(event);
                }
                return CliRunResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                };
            }
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
    if !events.is_empty() {
        ok_text(text_from_app_events(&events))
    } else {
        ok_text(python_json_dumps(&payload))
    }
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

fn models_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(models_help());
    }
    let format = value_for(args, &["--format"]).unwrap_or_else(|| "table".to_string());
    let verbose = has_flag(args, &["--verbose"]);
    let catalog_requested = has_flag(args, &["--catalog", "--providers"]);
    let cache = ensure_models_cache(args);
    let provider = value_for(args, &["--provider"]).or_else(|| {
        positional_args(
            args,
            &[
                "--format",
                "--ttl-seconds",
                "--models-url",
                "--model",
                "-m",
                "--provider",
            ],
        )
        .first()
        .cloned()
    });
    if catalog_requested {
        let payload = models_catalog_payload(&cache);
        if format == "json" {
            return CliRunResult::ok_json(&payload);
        }
        return ok_text(models_catalog_text(&payload, verbose));
    }
    let provider = provider.unwrap_or_else(active_provider);
    let normalized = match normalize_provider(Some(&provider)) {
        Ok(provider) => provider,
        Err(error) => return err_text(2, error),
    };
    let cached = load_cached_provider_models_from_cache(&cache.value, &normalized);
    let models = if let Some(models) = cached.filter(|items| !items.is_empty()) {
        models
    } else {
        let model_id = value_for(args, &["--model", "-m"])
            .or_else(|| provider_env_value(&normalized, "model"))
            .unwrap_or_else(|| default_model_for_provider(&normalized));
        vec![if normalized == "anthropic" {
            serde_json::to_value(anthropic_model(&model_id, 200_000, 8192))
                .unwrap_or_else(|_| json!({}))
        } else {
            serde_json::to_value(openai_compatible_model(&normalized, &model_id))
                .unwrap_or_else(|_| json!({}))
        }]
    };
    let provider_info = provider_catalog_record(&cache.value, &normalized)
        .unwrap_or_else(|| fallback_provider_catalog_record(&normalized, models.len()));
    let payload = json!({
        "provider": normalized,
        "provider_label": provider_label(&provider).unwrap_or(provider),
        "provider_info": provider_info,
        "models": models,
        "cache": cache.to_value(),
        "cache_path": models_cache_path().to_string_lossy(),
        "refreshed": cache.refreshed,
        "stale": cache.stale,
        "fallback": cache.fallback,
    });
    if format == "json" {
        CliRunResult::ok_json(&payload)
    } else {
        let rows = payload["models"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|model| {
                let id = model.get("id").and_then(Value::as_str).unwrap_or("-");
                if verbose {
                    let context = model
                        .get("context_window")
                        .or_else(|| model.get("limit").and_then(|limit| limit.get("context")))
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    let output = model
                        .get("max_output")
                        .or_else(|| model.get("limit").and_then(|limit| limit.get("output")))
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    let capabilities = model.get("capabilities").unwrap_or(&Value::Null);
                    format!(
                        "{id}  ctx={context} out={output} caps={}\n{}",
                        compact_capabilities(capabilities),
                        python_json_dumps(model)
                    )
                } else {
                    id.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        ok_text(format!(
            "provider: {}\ncache: {} ({})\n{}",
            payload["provider"].as_str().unwrap_or("openai"),
            cache.status,
            cache.path.to_string_lossy(),
            rows
        ))
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

fn mcp_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(mcp_help());
    }
    match args[0].as_str() {
        "add" => mcp_add(&args[1..]),
        "list" | "ls" => mcp_list(&args[1..]),
        "show" => mcp_show(&args[1..]),
        "remove" | "rm" => mcp_remove(&args[1..]),
        "auth" => mcp_auth(&args[1..]),
        "logout" => mcp_logout(&args[1..]),
        "doctor" => mcp_doctor(&args[1..]),
        "debug" => mcp_debug(&args[1..]),
        _ => err_text(2, format!("unknown mcp command: {}", args[0])),
    }
}

fn mcp_add(args: &[String]) -> CliRunResult {
    let positionals = positional_args(
        args,
        &[
            "--config",
            "--workspace",
            "--dir",
            "--url",
            "--transport",
            "--header",
            "--timeout-ms",
            "--format",
        ],
    );
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp add requires a server name");
    };
    let Some(url) = value_for(args, &["--url"]) else {
        return err_text(2, "mcp add requires --url");
    };
    let config_path = mcp_config_path(args);
    let mut config = read_json_file(&config_path);
    let servers = ensure_object_field(&mut config, "mcp");
    let headers = parse_headers(&values_for(args, &["--header"]));
    let server = json!({
        "type": "remote",
        "url": url,
        "transport": value_for(args, &["--transport"]).unwrap_or_else(|| "auto".to_string()),
        "enabled": !has_flag(args, &["--disabled"]),
        "timeout_ms": value_for(args, &["--timeout-ms"]).and_then(|value| value.parse::<u64>().ok()).unwrap_or(30_000),
        "headers": headers,
    });
    servers.insert(name.clone(), server);
    let public_server = mcp_public_server(name, servers.get(name).unwrap_or(&Value::Null));
    if let Err(error) = write_json_file(&config_path, &config) {
        return err_text(1, error);
    }
    let payload = json!({"config_path": config_path.to_string_lossy(), "server": public_server, "updated": true});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!("updated MCP server {name}"))
    }
}

fn mcp_list(args: &[String]) -> CliRunResult {
    let config_path = mcp_config_path(args);
    let servers = mcp_public_servers(&read_json_file(&config_path));
    let payload = json!({"config_path": config_path.to_string_lossy(), "servers": servers});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else if payload["servers"].as_array().is_none_or(Vec::is_empty) {
        ok_text("No MCP servers configured")
    } else {
        let lines = payload["servers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|server| {
                format!(
                    "{}  {}  {}  {}",
                    server["name"].as_str().unwrap_or(""),
                    server["enabled"].as_bool().unwrap_or(false),
                    server["transport"].as_str().unwrap_or("auto"),
                    server["url"].as_str().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        ok_text(format!("name  enabled  transport  url\n{lines}"))
    }
}

fn mcp_show(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--config", "--workspace", "--dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp show requires a server name");
    };
    let config_path = mcp_config_path(args);
    let config = read_json_file(&config_path);
    let server = config
        .get("mcp")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(name));
    let Some(server) = server else {
        return err_text(1, format!("MCP server not found: {name}"));
    };
    let payload = json!({"config_path": config_path.to_string_lossy(), "server": mcp_public_server(name, server)});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!(
            "{} {}",
            name,
            payload["server"]["url"].as_str().unwrap_or("")
        ))
    }
}

fn mcp_remove(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--config", "--workspace", "--dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp remove requires a server name");
    };
    let config_path = mcp_config_path(args);
    let mut config = read_json_file(&config_path);
    let removed = config
        .get_mut("mcp")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.remove(name))
        .is_some();
    if let Err(error) = write_json_file(&config_path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"config_path": config_path.to_string_lossy(), "name": name, "removed": removed}),
    )
}

fn mcp_auth(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(
            "Usage: openagent mcp auth <list|status|login|set-token|callback> [options]",
        );
    }
    match args[0].as_str() {
        "list" | "ls" | "status" => mcp_doctor(&args[1..]),
        "login" | "start" => mcp_auth_login(&args[1..]),
        "callback" => mcp_auth_callback(&args[1..]),
        "set-token" => {
            let positionals = positional_args(
                &args[1..],
                &[
                    "--config",
                    "--workspace",
                    "--dir",
                    "--bearer-token",
                    "--header-name",
                    "--format",
                ],
            );
            let Some(name) = positionals.first() else {
                return err_text(2, "mcp auth set-token requires a server name");
            };
            let Some(token) = value_for(&args[1..], &["--bearer-token"]) else {
                return err_text(
                    2,
                    "mcp auth set-token requires --bearer-token in this Rust CLI path",
                );
            };
            let header = value_for(&args[1..], &["--header-name"])
                .unwrap_or_else(|| "Authorization".to_string());
            let config_path = mcp_config_path(&args[1..]);
            let mut config = read_json_file(&config_path);
            let Some(server) = config
                .get_mut("mcp")
                .and_then(Value::as_object_mut)
                .and_then(|servers| servers.get_mut(name))
                .and_then(Value::as_object_mut)
            else {
                return err_text(1, format!("MCP server not found: {name}"));
            };
            let headers = server.entry("headers").or_insert_with(|| json!({}));
            if let Some(headers) = headers.as_object_mut() {
                headers.insert(header.clone(), json!(format!("Bearer {token}")));
            }
            if let Err(error) = write_json_file(&config_path, &config) {
                return err_text(1, error);
            }
            CliRunResult::ok_json(
                &json!({"config_path": config_path.to_string_lossy(), "name": name, "header": header, "updated": true}),
            )
        }
        _ => err_text(2, format!("unknown mcp auth command: {}", args[0])),
    }
}

fn mcp_auth_login(args: &[String]) -> CliRunResult {
    let positionals = positional_args(
        args,
        &[
            "--config",
            "--workspace",
            "--dir",
            "--client-id",
            "--client-secret",
            "--authorize-url",
            "--token-url",
            "--redirect-uri",
            "--scope",
            "--format",
        ],
    );
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp auth login requires a server name");
    };
    let config_path = mcp_config_path(args);
    let mut config = read_json_file(&config_path);
    let Some(server) = config
        .get_mut("mcp")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.get_mut(name))
        .and_then(Value::as_object_mut)
    else {
        return err_text(1, format!("MCP server not found: {name}"));
    };
    let state = new_cli_id("mcp_oauth");
    let redirect_uri = value_for(args, &["--redirect-uri"])
        .unwrap_or_else(|| "http://127.0.0.1:8787/mcp/oauth/callback".to_string());
    let authorize_url = value_for(args, &["--authorize-url"])
        .or_else(|| {
            server
                .get("authorize_url")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            let url = server
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("{}/authorize", url.trim_end_matches('/'))
        });
    let client_id =
        value_for(args, &["--client-id"]).unwrap_or_else(|| "openagent-cli".to_string());
    let scope = value_for(args, &["--scope"]).unwrap_or_else(|| "mcp".to_string());
    let url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
        authorize_url,
        url_encode(&client_id),
        url_encode(&redirect_uri),
        url_encode(&scope),
        url_encode(&state)
    );
    server.insert(
        "oauth".to_string(),
        json!({
            "state": state,
            "client_id": client_id,
            "client_secret": value_for(args, &["--client-secret"]).unwrap_or_default(),
            "authorize_url": authorize_url,
            "token_url": value_for(args, &["--token-url"]),
            "redirect_uri": redirect_uri,
            "scope": scope,
            "status": "authorization_required",
            "updated_at_ms": now_ms_cli(),
        }),
    );
    if let Err(error) = write_json_file(&config_path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(&json!({
        "config_path": config_path.to_string_lossy(),
        "name": name,
        "status": "authorization_required",
        "authorize_url": url,
    }))
}

fn mcp_auth_callback(args: &[String]) -> CliRunResult {
    let positionals = positional_args(
        args,
        &[
            "--config",
            "--workspace",
            "--dir",
            "--code",
            "--state",
            "--access-token",
            "--format",
        ],
    );
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp auth callback requires a server name");
    };
    let config_path = mcp_config_path(args);
    let mut config = read_json_file(&config_path);
    let Some(server) = config
        .get_mut("mcp")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.get_mut(name))
        .and_then(Value::as_object_mut)
    else {
        return err_text(1, format!("MCP server not found: {name}"));
    };
    let expected_state = server
        .get("oauth")
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(state) = value_for(args, &["--state"])
        && !expected_state.is_empty()
        && state != expected_state
    {
        return err_text(1, "MCP OAuth state mismatch");
    }
    let access_token = value_for(args, &["--access-token"]).unwrap_or_else(|| {
        value_for(args, &["--code"])
            .map(|code| format!("code:{code}"))
            .unwrap_or_default()
    });
    if access_token.is_empty() {
        return err_text(2, "mcp auth callback requires --code or --access-token");
    }
    let headers = server.entry("headers").or_insert_with(|| json!({}));
    if let Some(headers) = headers.as_object_mut() {
        headers.insert(
            "Authorization".to_string(),
            json!(format!("Bearer {access_token}")),
        );
    }
    server.insert(
        "oauth".to_string(),
        json!({"status": "authorized", "updated_at_ms": now_ms_cli()}),
    );
    if let Err(error) = write_json_file(&config_path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"config_path": config_path.to_string_lossy(), "name": name, "status": "authorized"}),
    )
}

fn mcp_logout(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--config", "--workspace", "--dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp logout requires a server name");
    };
    let config_path = mcp_config_path(args);
    let mut config = read_json_file(&config_path);
    let removed = config
        .get_mut("mcp")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.get_mut(name))
        .and_then(Value::as_object_mut)
        .and_then(|server| server.get_mut("headers"))
        .and_then(Value::as_object_mut)
        .and_then(|headers| headers.remove("Authorization"))
        .is_some();
    if let Err(error) = write_json_file(&config_path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"config_path": config_path.to_string_lossy(), "name": name, "removed": removed}),
    )
}

fn mcp_doctor(args: &[String]) -> CliRunResult {
    let config_path = mcp_config_path(args);
    let config = if config_path.exists() {
        match load_mcp_config(&config_path.to_string_lossy()) {
            Ok(config) => config,
            Err(error) => return err_text(1, error),
        }
    } else {
        openagent_mcp::McpConfig::default()
    };
    let refresh = has_flag(args, &["--refresh"]);
    let mut manager = RemoteMcpManager::new(config.clone());
    let mut refresh_error = None::<String>;
    if refresh {
        for server in config.servers.iter().filter(|server| server.enabled) {
            match discover_mcp_server_tools(server) {
                Ok((transport, tools)) => {
                    let descriptors = build_tool_descriptors_from_values(server, &tools);
                    let _ = manager.set_server_tools(
                        &server.name,
                        Some(transport),
                        "connected",
                        Some(now_ms_cli() as f64 / 1000.0),
                        descriptors,
                    );
                }
                Err(error) => {
                    refresh_error = Some(error);
                }
            }
        }
    }
    let snapshot = serde_json::to_value(manager.snapshot()).unwrap_or_else(|_| json!({}));
    let servers = snapshot
        .get("servers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let payload = json!({
        "config_path": config_path.to_string_lossy(),
        "configured": !servers.is_empty(),
        "enabled": servers.iter().any(|server| server["enabled"].as_bool().unwrap_or(false)),
        "server_count": servers.len(),
        "ok": refresh_error.is_none() && servers.iter().all(|server| server["status"].as_str() != Some("failed")),
        "refresh_error": refresh_error,
        "servers": servers,
    });
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!(
            "{} MCP server(s)",
            payload["server_count"].as_u64().unwrap_or(0)
        ))
    }
}

fn mcp_debug(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--config", "--workspace", "--dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "mcp debug requires a server name");
    };
    let config_path = mcp_config_path(args);
    let config = read_json_file(&config_path);
    let server = config
        .get("mcp")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(name));
    let Some(server) = server else {
        return err_text(1, format!("MCP server not found: {name}"));
    };
    CliRunResult::ok_json(&json!({"server": mcp_public_server(name, server)}))
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
        ok_text(format!(
            "{} session(s)",
            payload["sessions"].as_array().map_or(0, Vec::len)
        ))
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
        ok_text(format!("{session_count} session(s), {run_count} run(s)"))
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

#[derive(Clone, Debug)]
struct ProviderRunResult {
    answer: String,
    tool_calls: Vec<ToolCall>,
    usage: Usage,
    source: String,
    finish_reason: String,
}

#[derive(Debug)]
struct AgentLoopOutcome {
    answer: String,
    usage: Usage,
    source: String,
    events: Vec<Value>,
    steps: u64,
    tool_calls: u64,
    finish_reason: String,
}

#[derive(Debug)]
struct AgentLoopError {
    message: String,
    events: Vec<Value>,
    steps: u64,
    finish_reason: Option<String>,
    paused: bool,
}

#[derive(Clone, Debug, Default)]
struct RunAgentProfile {
    id: String,
    name: String,
    mode: String,
    model: Option<String>,
    provider: Option<String>,
    permission: Option<String>,
    prompt: Option<String>,
    tools: Vec<String>,
    source_path: Option<PathBuf>,
    loaded: bool,
}

#[derive(Clone, Debug)]
struct PendingResume {
    kind: String,
    request_id: String,
    call: ToolCall,
    response: Value,
    step: u64,
}

#[derive(Clone, Debug)]
struct McpRuntime {
    manager: RemoteMcpManager,
    descriptors: BTreeMap<String, RemoteMcpToolDescriptor>,
    snapshot: Value,
}

#[derive(Clone, Debug, Default)]
struct RemoteAuth {
    token: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

fn remote_auth_from_args(args: &[String]) -> RemoteAuth {
    RemoteAuth {
        token: value_for(args, &["--server-token"])
            .or_else(|| env::var(DEFAULT_SERVER_TOKEN_ENV).ok())
            .or_else(|| {
                value_for(args, &["--server-token-env"]).and_then(|name| env::var(name).ok())
            }),
        username: value_for(args, &["--username", "-u"]),
        password: value_for(args, &["--password", "-p"]),
    }
}

struct AgentLoopRequest<'a> {
    args: &'a [String],
    workspace: &'a Path,
    provider: &'a str,
    model_id: &'a str,
    session: &'a mut Session,
    store: &'a FileSessionStore,
    run_id: &'a str,
    max_steps: u64,
    prompt: &'a str,
    agent_profile: Option<&'a RunAgentProfile>,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
}

#[derive(Debug)]
struct RunSessionSelection {
    store: FileSessionStore,
    session: Session,
    forked: bool,
}

fn read_piped_stdin() -> String {
    if io::stdin().is_terminal() {
        return String::new();
    }
    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);
    input
}

fn build_run_prompt(
    args: &[String],
    workspace: &Path,
    _agent_profile: Option<&RunAgentProfile>,
    allow_empty_resume: bool,
) -> Result<(String, String, bool), String> {
    let message_args = positional_args(args, RUN_POSITIONAL_VALUE_FLAGS);
    let message = message_args.join(" ");
    let stdin_text = read_piped_stdin();
    let message = if message.trim().is_empty() {
        stdin_text.trim().to_string()
    } else if stdin_text.trim().is_empty() {
        message
    } else {
        format!("{}\n{}", message.trim(), stdin_text.trim())
    };
    if message.trim().is_empty() {
        if allow_empty_resume {
            return Ok((String::new(), String::new(), false));
        }
        return Err("openagent run requires a prompt or piped stdin".to_string());
    }
    let files = attached_files(workspace, &values_for(args, &["--file", "-f"]))?;
    let prompt = if let Some(command_name) = value_for(args, &["--command"]) {
        let mut command_args = message_args;
        if !stdin_text.trim().is_empty() {
            command_args.push(stdin_text.trim().to_string());
        }
        let command = discover_custom_commands(args)
            .into_iter()
            .find(|item| item.name == command_name)
            .ok_or_else(|| format!("Command not found: {command_name}"))?;
        let rendered = render_custom_template(&command.template, &command_args, workspace);
        build_prompt_with_files(&rendered, &files)
    } else {
        build_prompt_with_files(&message, &files)
    };
    Ok((message, prompt, true))
}

fn err_json_events(
    mut events: Vec<Value>,
    message: String,
    status: &str,
    event_sink: &mut Option<&mut dyn FnMut(&Value)>,
) -> CliRunResult {
    let failed = json!({
        "method": "turn/completed",
        "params": {
            "status": status,
            "error": message,
        }
    });
    if let Some(emit) = event_sink.as_deref_mut() {
        emit(&failed);
        return CliRunResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        };
    }
    events.push(failed);
    CliRunResult {
        exit_code: 1,
        stdout: ensure_trailing_newline(
            events
                .iter()
                .map(python_json_dumps)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        stderr: String::new(),
    }
}

fn provider_and_model_from_args(
    args: &[String],
    agent_profile: Option<&RunAgentProfile>,
) -> (String, String) {
    if let Some(raw) = value_for(args, &["--model", "-m"])
        && let Some((provider, model)) = raw.split_once('/')
        && !provider.is_empty()
        && !model.is_empty()
    {
        let provider = normalize_provider(Some(provider)).unwrap_or_else(|_| provider.to_string());
        return (provider, model.to_string());
    }
    if value_for(args, &["--model", "-m"]).is_none()
        && let Some(raw) = agent_profile.and_then(|profile| profile.model.as_deref())
        && let Some((provider, model)) = raw.split_once('/')
        && !provider.is_empty()
        && !model.is_empty()
    {
        let provider = normalize_provider(Some(provider)).unwrap_or_else(|_| provider.to_string());
        return (provider, model.to_string());
    }
    let provider = value_for(args, &["--provider"])
        .or_else(|| agent_profile.and_then(|profile| profile.provider.clone()))
        .unwrap_or_else(active_provider);
    let provider = normalize_provider(Some(&provider)).unwrap_or(provider);
    let model = value_for(args, &["--model", "-m"])
        .or_else(|| agent_profile.and_then(|profile| profile.model.clone()))
        .or_else(|| provider_env_value(&provider, "model"))
        .unwrap_or_else(|| default_model_for_provider(&provider));
    (provider, model)
}

fn load_agent_profile_from_args(
    args: &[String],
    _workspace: &Path,
) -> Result<Option<RunAgentProfile>, String> {
    let Some(raw_name) = value_for(args, &["--agent"]) else {
        return Ok(None);
    };
    let agent_id = sanitize_identifier(&raw_name);
    let path = agent_registry_dir(args).join(format!("{agent_id}.json"));
    let value = read_json_file(&path);
    if value.as_object().is_none_or(Map::is_empty) {
        return Err(format!(
            "agent profile not found: {raw_name} ({})",
            path.display()
        ));
    }
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("primary")
        .to_ascii_lowercase();
    if !matches!(mode.as_str(), "primary" | "subagent") {
        return Err(format!(
            "agent profile {agent_id} has invalid mode '{mode}'; expected primary or subagent"
        ));
    }
    Ok(Some(RunAgentProfile {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .map(sanitize_identifier)
            .unwrap_or(agent_id),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&raw_name)
            .to_string(),
        mode,
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        provider: value
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string),
        permission: value
            .get("permission")
            .and_then(Value::as_str)
            .map(str::to_string),
        prompt: value
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::to_string),
        tools: value
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        source_path: Some(path),
        loaded: true,
    }))
}

fn agent_profile_public_value(profile: &RunAgentProfile) -> Value {
    json!({
        "id": profile.id.clone(),
        "name": profile.name.clone(),
        "mode": profile.mode.clone(),
        "model": profile.model.clone(),
        "provider": profile.provider.clone(),
        "permission": profile.permission.clone(),
        "tools": profile.tools.clone(),
        "loaded": profile.loaded,
        "source_path": profile.source_path.as_ref().map(|path| path.to_string_lossy().to_string()),
    })
}

fn bind_agent_profile_system_prompt(
    session: &mut Session,
    store: &FileSessionStore,
    run_id: &str,
    profile: Option<&RunAgentProfile>,
) -> Result<(), String> {
    let Some(profile) = profile else {
        return Ok(());
    };
    let Some(prompt) = profile
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let already_bound = session.messages.iter().any(|message| {
        message.role == Role::System
            && message
                .metadata
                .get("agent_profile")
                .and_then(Value::as_str)
                == Some(profile.id.as_str())
    });
    if already_bound {
        return Ok(());
    }
    let mut message = chat_message(Role::System, prompt.to_string());
    message
        .metadata
        .insert("agent_profile".to_string(), json!(profile.id.clone()));
    message
        .metadata
        .insert("agent_mode".to_string(), json!(profile.mode.clone()));
    let index = session.messages.len() as u64;
    session.add(message.clone());
    store
        .append_message(session, &message, run_id, index)
        .map_err(|error| format!("failed to record agent system prompt: {error}"))
}

fn title_from_args(args: &[String], message: &str) -> Option<String> {
    let title = value_for(args, &["--title"])?;
    if title.is_empty() {
        let compact = message.split_whitespace().collect::<Vec<_>>().join(" ");
        return Some(if compact.chars().count() > 50 {
            format!("{}...", compact.chars().take(50).collect::<String>())
        } else {
            compact
        });
    }
    Some(title)
}

fn prepare_run_session(
    args: &[String],
    workspace: &Path,
    provider: &str,
    model_id: &str,
) -> Result<RunSessionSelection, String> {
    let root = session_root_from_args(args);
    let store = FileSessionStore::new(root.clone());
    let explicit = value_for(args, &["--session", "-s"]);
    let continue_last = has_flag(args, &["--continue", "-c"]);
    let fork = has_flag(args, &["--fork"]);
    if fork && explicit.is_none() && !continue_last {
        return Err("--fork requires --continue or --session".to_string());
    }
    let base_id = explicit.or_else(|| continue_last.then(|| latest_session_id(&root)).flatten());
    let mut session = if let Some(session_id) = base_id {
        if !valid_session_id(&session_id) {
            return Err("Invalid session id".to_string());
        }
        store
            .load_session(&session_id)
            .map_err(|error| error.to_string())?
    } else {
        Session::new(new_cli_id("session"), workspace)
    };
    let forked = fork;
    if forked {
        let mut forked_session = Session::new(new_cli_id("session"), workspace);
        forked_session.messages = session.messages.clone();
        forked_session.todos = session.todos.clone();
        forked_session.metadata = session.metadata.clone();
        forked_session
            .metadata
            .insert("forked_from".to_string(), json!(session.id.clone()));
        session = forked_session;
    }
    session
        .metadata
        .insert("provider".to_string(), json!(provider));
    session
        .metadata
        .insert("model".to_string(), json!(model_id));
    Ok(RunSessionSelection {
        store,
        session,
        forked,
    })
}

fn run_agent_loop(
    request: AgentLoopRequest<'_>,
    event_sink: &mut Option<&mut dyn FnMut(&Value)>,
) -> Result<AgentLoopOutcome, AgentLoopError> {
    let AgentLoopRequest {
        args,
        workspace,
        provider,
        model_id,
        session,
        store,
        run_id,
        max_steps,
        prompt,
        agent_profile,
        permission_ruleset,
        skip_permissions,
    } = request;
    let mut toolkit = Toolkit::with_builtins();
    let mcp_runtime = load_mcp_runtime(args, &mut toolkit).map_err(|message| AgentLoopError {
        message,
        events: Vec::new(),
        steps: 0,
        finish_reason: Some("mcp_discovery_error".to_string()),
        paused: false,
    })?;
    let tools = filter_tools_for_agent(toolkit.get_all_tools("local"), agent_profile);
    let mut ctx = ToolContext::new(workspace)
        .with_session_id(session.id.clone())
        .with_permission_ruleset(permission_ruleset)
        .with_dangerously_skip_permissions(skip_permissions);
    if let Some(answers) = configured_question_answers(args) {
        ctx.set_question_answers(answers);
    }
    if let Some(runtime) = mcp_runtime.as_ref() {
        let _ = store.record_event(
            &session.id,
            run_id,
            "mcp.discovery",
            SessionEventOptions {
                kind: "mcp".to_string(),
                attributes: BTreeMap::from([(
                    "snapshot".to_string(),
                    sanitize_mcp_observation_value(&runtime.snapshot),
                )]),
                ..SessionEventOptions::default()
            },
        );
    }

    let mut answer = String::new();
    let mut events = Vec::new();
    let mut total_usage = Usage::default();
    let mut total_tool_calls = 0_u64;
    let mut first_delta = true;
    let mut approval_always = approval_always_patterns(session);

    if let Some(pending) = pending_resume_from_session(session) {
        total_tool_calls += 1;
        let mut resume_context = PendingResumeContext {
            toolkit: &toolkit,
            mcp_runtime: mcp_runtime.as_ref(),
            ctx: &mut ctx,
            session,
            store,
            run_id,
            events: &mut events,
            event_sink,
        };
        process_pending_resume(pending, &mut resume_context).map_err(|message| AgentLoopError {
            message,
            events: events.clone(),
            steps: 0,
            finish_reason: Some("resume_error".to_string()),
            paused: false,
        })?;
        approval_always = approval_always_patterns(session);
    }

    for step in 1..=max_steps {
        let mut streamed_events = Vec::new();
        let provider_messages = store
            .materialized_chat_messages(session)
            .unwrap_or_else(|_| session.messages.clone());
        let mut on_provider_stream = |event: &ProviderStreamEvent| {
            if let ProviderStreamEvent::TextDelta { text } = event
                && !text.is_empty()
            {
                let mut params = json!({
                    "delta": text,
                    "session_id": session.id.clone(),
                    "run_id": run_id,
                    "step": step,
                });
                if first_delta {
                    params["prompt"] = json!(prompt);
                    first_delta = false;
                }
                emit_run_event(
                    &mut streamed_events,
                    json!({"method": "item/agentMessage/delta", "params": params}),
                    event_sink,
                );
            }
        };
        let provider_result = call_provider_for_run(
            args,
            provider,
            model_id,
            &provider_messages,
            &tools,
            Some(&mut on_provider_stream),
        )
        .map_err(|message| AgentLoopError {
            message,
            events: events.clone(),
            steps: step,
            finish_reason: Some("provider_error".to_string()),
            paused: false,
        })?;
        let streamed_text = !streamed_events.is_empty();
        events.extend(streamed_events);
        let source = provider_result.source.clone();
        add_usage(&mut total_usage, &provider_result.usage);
        let step_text = provider_result.answer.clone();
        if !step_text.is_empty() {
            answer.push_str(&step_text);
            if !streamed_text {
                let mut params = json!({
                    "delta": step_text,
                    "session_id": session.id.clone(),
                    "run_id": run_id,
                    "step": step,
                });
                if first_delta {
                    params["prompt"] = json!(prompt);
                    first_delta = false;
                }
                emit_run_event(
                    &mut events,
                    json!({"method": "item/agentMessage/delta", "params": params}),
                    event_sink,
                );
            }
            store
                .append_part(
                    &session.id,
                    run_id,
                    "text",
                    SessionPartOptions {
                        attributes: BTreeMap::from([
                            ("role".to_string(), json!("assistant")),
                            ("chars".to_string(), json!(step_text.chars().count())),
                        ]),
                        step_index: Some(step),
                        ..SessionPartOptions::default()
                    },
                )
                .map_err(|error| AgentLoopError {
                    message: format!("failed to record assistant text part: {error}"),
                    events: events.clone(),
                    steps: step,
                    finish_reason: Some("store_error".to_string()),
                    paused: false,
                })?;
        }

        let assistant_index = session.messages.len() as u64;
        let assistant_message_id = cli_message_id(assistant_index);
        let mut assistant_message =
            assistant_message_for_provider_step(step_text, &provider_result.tool_calls);
        assistant_message.metadata.insert(
            "message_id".to_string(),
            json!(assistant_message_id.clone()),
        );
        assistant_message
            .metadata
            .insert("step".to_string(), json!(step));
        session.add(assistant_message.clone());
        store
            .append_message(session, &assistant_message, run_id, assistant_index)
            .map_err(|error| AgentLoopError {
                message: format!("failed to record assistant message: {error}"),
                events: events.clone(),
                steps: step,
                finish_reason: Some("store_error".to_string()),
                paused: false,
            })?;

        if provider_result.tool_calls.is_empty() {
            record_step_finished(
                store,
                &session.id,
                run_id,
                step,
                &provider_result.finish_reason,
                0,
                &provider_result.usage,
            );
            return Ok(AgentLoopOutcome {
                answer,
                usage: total_usage,
                source,
                events,
                steps: step,
                tool_calls: total_tool_calls,
                finish_reason: provider_result.finish_reason,
            });
        }

        for tool_call in provider_result.tool_calls {
            total_tool_calls += 1;
            emit_run_event(
                &mut events,
                json!({
                    "method": "item/toolCall/started",
                    "params": {
                        "session_id": session.id.clone(),
                        "run_id": run_id,
                        "step": step,
                        "call_id": tool_call.call_id.clone(),
                        "name": tool_call.name.clone(),
                        "input": tool_call.input.clone(),
                    }
                }),
                event_sink,
            );
            let _ = store.record_event(
                &session.id,
                run_id,
                "tool.call.started",
                SessionEventOptions {
                    kind: "tool".to_string(),
                    attributes: BTreeMap::from([
                        ("call_id".to_string(), json!(tool_call.call_id.clone())),
                        ("name".to_string(), json!(tool_call.name.clone())),
                        ("input".to_string(), tool_call.input.clone()),
                        ("step".to_string(), json!(step)),
                    ]),
                    ..SessionEventOptions::default()
                },
            );

            if tool_call.name == "question" && ctx.question_answers.is_none() {
                let message =
                    "question tool requires an answer; rerun with --answer or OPENAGENT_QUESTION_ANSWERS".to_string();
                emit_run_event(
                    &mut events,
                    json!({
                        "method": "turn/question_requested",
                        "params": {
                            "session_id": session.id.clone(),
                            "run_id": run_id,
                            "step": step,
                            "call_id": tool_call.call_id.clone(),
                            "questions": tool_call.input.get("questions").cloned().unwrap_or_else(|| json!([])),
                        }
                    }),
                    event_sink,
                );
                let _ = store.record_event(
                    &session.id,
                    run_id,
                    "question.requested",
                    SessionEventOptions {
                        kind: "question".to_string(),
                        attributes: BTreeMap::from([
                            ("call_id".to_string(), json!(tool_call.call_id.clone())),
                            (
                                "questions".to_string(),
                                tool_call
                                    .input
                                    .get("questions")
                                    .cloned()
                                    .unwrap_or_else(|| json!([])),
                            ),
                        ]),
                        ..SessionEventOptions::default()
                    },
                );
                let _ = store.append_part(
                    &session.id,
                    run_id,
                    "question",
                    SessionPartOptions {
                        message_id: Some(assistant_message_id.clone()),
                        content: Some(json!({
                            "call_id": tool_call.call_id.clone(),
                            "name": tool_call.name.clone(),
                            "questions": tool_call.input.get("questions").cloned().unwrap_or_else(|| json!([])),
                            "status": "pending",
                        })),
                        attributes: BTreeMap::from([
                            ("call_id".to_string(), json!(tool_call.call_id.clone())),
                            ("name".to_string(), json!(tool_call.name.clone())),
                        ]),
                        step_index: Some(step),
                        status: "pending".to_string(),
                        ..SessionPartOptions::default()
                    },
                );
                session.metadata.insert(
                    "pending_question".to_string(),
                    json!({
                        "request_id": format!("question_{}", tool_call.call_id),
                        "session_id": session.id.clone(),
                        "turn_id": run_id,
                        "run_id": run_id,
                        "step": step,
                        "call_id": tool_call.call_id.clone(),
                        "tool_name": tool_call.name.clone(),
                        "tool_input": tool_call.input.clone(),
                        "assistant_message_id": assistant_message_id.clone(),
                        "questions": tool_call.input.get("questions").cloned().unwrap_or_else(|| json!([])),
                        "created_at_ms": now_ms_cli(),
                    }),
                );
                session.metadata.remove("pending_question_response");
                let _ = store.save_state(session, Some(run_id));
                return Err(AgentLoopError {
                    message,
                    events,
                    steps: step,
                    finish_reason: Some("question_required".to_string()),
                    paused: true,
                });
            }

            let mut tool_result =
                execute_agent_tool(&toolkit, mcp_runtime.as_ref(), &tool_call, &mut ctx);
            if tool_result
                .metadata
                .get("requires_approval")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let pattern = tool_result
                    .metadata
                    .get("permission_pattern")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if approval_always.iter().any(|item| item == &pattern) {
                    let previous = ctx.dangerously_skip_permissions;
                    ctx.dangerously_skip_permissions = true;
                    tool_result =
                        execute_agent_tool(&toolkit, mcp_runtime.as_ref(), &tool_call, &mut ctx);
                    ctx.dangerously_skip_permissions = previous;
                }
            }
            if tool_result
                .metadata
                .get("requires_approval")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let message = format!(
                    "approval required for tool {} (call {})",
                    tool_call.name, tool_call.call_id
                );
                let mut approval = approval_payload_for_tool_call(
                    session,
                    run_id,
                    step,
                    &tool_call,
                    &tool_result.metadata,
                );
                if let Some(object) = approval.as_object_mut() {
                    object.insert(
                        "assistant_message_id".to_string(),
                        json!(assistant_message_id.clone()),
                    );
                }
                emit_run_event(
                    &mut events,
                    json!({
                        "method": "turn/approval_requested",
                        "params": {
                            "session_id": session.id.clone(),
                            "run_id": run_id,
                            "step": step,
                            "approval": approval,
                        }
                    }),
                    event_sink,
                );
                let _ = store.record_event(
                    &session.id,
                    run_id,
                    "approval.requested",
                    SessionEventOptions {
                        kind: "approval".to_string(),
                        attributes: BTreeMap::from([
                            ("call_id".to_string(), json!(tool_call.call_id.clone())),
                            ("name".to_string(), json!(tool_call.name.clone())),
                            (
                                "reason".to_string(),
                                json!(
                                    tool_result
                                        .metadata
                                        .get("error_kind")
                                        .and_then(Value::as_str)
                                        .unwrap_or("permission_required")
                                ),
                            ),
                            ("metadata".to_string(), json!(tool_result.metadata)),
                        ]),
                        ..SessionEventOptions::default()
                    },
                );
                let _ = store.append_part(
                    &session.id,
                    run_id,
                    "approval",
                    SessionPartOptions {
                        message_id: Some(assistant_message_id.clone()),
                        content: Some(json!({
                            "call_id": tool_call.call_id.clone(),
                            "name": tool_call.name.clone(),
                            "approval": approval.clone(),
                            "status": "pending",
                        })),
                        attributes: BTreeMap::from([
                            ("call_id".to_string(), json!(tool_call.call_id.clone())),
                            ("name".to_string(), json!(tool_call.name.clone())),
                        ]),
                        step_index: Some(step),
                        status: "pending".to_string(),
                        ..SessionPartOptions::default()
                    },
                );
                session
                    .metadata
                    .insert("pending_approval".to_string(), approval.clone());
                session.metadata.remove("pending_approval_response");
                let _ = store.save_state(session, Some(run_id));
                return Err(AgentLoopError {
                    message,
                    events,
                    steps: step,
                    finish_reason: Some("approval_required".to_string()),
                    paused: true,
                });
            }
            let failed = tool_result.error.is_some();
            let tool_output = tool_result.output.clone();
            let tool_error = tool_result.error.clone();
            let tool_metadata = tool_result.metadata.clone();
            emit_run_event(
                &mut events,
                json!({
                    "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
                    "params": {
                        "session_id": session.id.clone(),
                        "run_id": run_id,
                        "step": step,
                        "call_id": tool_call.call_id.clone(),
                        "name": tool_call.name.clone(),
                        "output": tool_output,
                        "error": tool_error,
                        "metadata": tool_metadata,
                    }
                }),
                event_sink,
            );
            let _ = store.record_event(
                &session.id,
                run_id,
                if failed {
                    "tool.call.failed"
                } else {
                    "tool.call.finished"
                },
                SessionEventOptions {
                    kind: "tool".to_string(),
                    status: if failed {
                        "error".to_string()
                    } else {
                        "ok".to_string()
                    },
                    attributes: BTreeMap::from([
                        ("call_id".to_string(), json!(tool_call.call_id.clone())),
                        ("name".to_string(), json!(tool_call.name.clone())),
                        ("error".to_string(), json!(tool_result.error.clone())),
                        ("metadata".to_string(), json!(tool_result.metadata.clone())),
                        ("step".to_string(), json!(step)),
                    ]),
                    ..SessionEventOptions::default()
                },
            );
            let _ = store.append_part(
                &session.id,
                run_id,
                "tool_result",
                SessionPartOptions {
                    attributes: BTreeMap::from([
                        ("call_id".to_string(), json!(tool_call.call_id.clone())),
                        ("name".to_string(), json!(tool_call.name.clone())),
                        ("failed".to_string(), json!(failed)),
                    ]),
                    step_index: Some(step),
                    ..SessionPartOptions::default()
                },
            );

            let mut tool_message = chat_message(
                Role::Tool,
                tool_result.error.as_ref().map_or_else(
                    || tool_result.output.clone(),
                    |error| format!("Tool failed: {error}"),
                ),
            );
            tool_message.name = Some(tool_call.name.clone());
            tool_message.tool_call_id = Some(tool_call.call_id.clone());
            tool_message
                .metadata
                .insert("tool_result".to_string(), json!(tool_result));
            tool_message.metadata.insert(
                "assistant_message_id".to_string(),
                json!(assistant_message_id.clone()),
            );
            tool_message
                .metadata
                .insert("step".to_string(), json!(step));
            let tool_index = session.messages.len() as u64;
            session.add(tool_message.clone());
            store
                .append_message(session, &tool_message, run_id, tool_index)
                .map_err(|error| AgentLoopError {
                    message: format!("failed to record tool message: {error}"),
                    events: events.clone(),
                    steps: step,
                    finish_reason: Some("store_error".to_string()),
                    paused: false,
                })?;
        }

        record_step_finished(
            store,
            &session.id,
            run_id,
            step,
            "tool_call",
            total_tool_calls,
            &provider_result.usage,
        );
    }

    Err(AgentLoopError {
        message: format!("agent loop exceeded max steps ({max_steps})"),
        events,
        steps: max_steps,
        finish_reason: Some("max_steps".to_string()),
        paused: false,
    })
}

fn call_provider_for_run(
    args: &[String],
    provider: &str,
    model_id: &str,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
) -> Result<ProviderRunResult, String> {
    if !messages.iter().any(|message| message.role == Role::Tool)
        && let Some(tool_calls) = mock_tool_calls_from_env()?
    {
        return Ok(ProviderRunResult {
            answer: env::var("OPENAGENT_MOCK_TOOL_PREFACE").unwrap_or_default(),
            tool_calls,
            usage: Usage::default(),
            source: "mock".to_string(),
            finish_reason: "tool_call".to_string(),
        });
    }
    if let Ok(answer) = env::var("OPENAGENT_MOCK_ANSWER")
        && !answer.is_empty()
    {
        return Ok(ProviderRunResult {
            answer,
            tool_calls: Vec::new(),
            usage: Usage::default(),
            source: "mock".to_string(),
            finish_reason: "stop".to_string(),
        });
    }
    let api_key = provider_api_key(provider, args);
    if provider_requires_api_key(provider).unwrap_or(true) && api_key.is_none() {
        return Ok(ProviderRunResult {
            answer: "hello from openagent".to_string(),
            tool_calls: Vec::new(),
            usage: Usage::default(),
            source: "offline_fallback_missing_api_key".to_string(),
            finish_reason: "stop".to_string(),
        });
    }
    let api_key = api_key.unwrap_or_default();
    if provider == "anthropic" {
        call_anthropic_provider(args, &api_key, model_id, messages, tools, stream_sink)
    } else {
        call_openai_compatible_provider(
            args,
            provider,
            &api_key,
            model_id,
            messages,
            tools,
            stream_sink,
        )
    }
}

fn call_openai_compatible_provider(
    args: &[String],
    provider: &str,
    api_key: &str,
    model_id: &str,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    mut stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
) -> Result<ProviderRunResult, String> {
    let base_url = provider_base_url(provider, args);
    if is_synthetic_endpoint(&base_url) {
        return Ok(ProviderRunResult {
            answer: "hello from openagent".to_string(),
            tool_calls: Vec::new(),
            usage: Usage::default(),
            source: "offline_fallback_synthetic_endpoint".to_string(),
            finish_reason: "stop".to_string(),
        });
    }
    let wire_api = provider_wire_api(provider, args);
    let timeout = Duration::from_secs(
        value_for(args, &["--timeout-s"])
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(60),
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())?;
    let mut config = OpenAiLanguageModelConfig::new(api_key, model_id);
    config.provider_id = provider.to_string();
    config.base_url = base_url.clone();
    config.wire_api = wire_api.clone();
    config.reasoning_effort = value_for(args, &["--variant"]);
    let stream = provider_streaming_enabled(args);
    let (endpoint, mut payload) = if wire_api == "chat" {
        let mut payload =
            build_openai_chat_payload(&config, None, messages, tools, None, None, None);
        if let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(stream));
        }
        (join_url(&base_url, "chat/completions"), payload)
    } else {
        let mut payload =
            build_openai_responses_payload(&config, None, messages, tools, None, None);
        if stream && let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(true));
        }
        (join_url(&base_url, "responses"), payload)
    };
    if let Some(max_tokens) =
        value_for(args, &["--max-output-tokens"]).and_then(|value| value.parse::<u64>().ok())
        && let Some(object) = payload.as_object_mut()
    {
        object.insert(
            if wire_api == "chat" {
                "max_tokens"
            } else {
                "max_output_tokens"
            }
            .to_string(),
            json!(max_tokens),
        );
    }
    let mut request = client
        .post(endpoint)
        .bearer_auth(api_key)
        .header("content-type", "application/json");
    if stream {
        request = request.header("accept", "text/event-stream");
    }
    let response = request
        .json(&payload)
        .send()
        .map_err(|error| format!("provider request failed: {error}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if stream {
        if !status.is_success() {
            let raw = response
                .text()
                .map_err(|error| format!("provider response read failed: {error}"))?;
            return Err(format!(
                "provider returned HTTP {}: {}",
                status.as_u16(),
                summarize_http_error_body(&raw, &content_type)
            ));
        }
        let mut chunks = Vec::new();
        read_sse_json_values_stream(response, |chunk| {
            if let Some(event) = openai_stream_text_delta(&wire_api, &chunk)
                && let Some(sink) = stream_sink.as_deref_mut()
            {
                sink(&event);
            }
            chunks.push(chunk);
            Ok(())
        })?;
        let events = if wire_api == "chat" {
            normalize_openai_chat_sse_chunks(&chunks)
        } else {
            normalize_openai_responses_stream_events(&chunks)
        };
        return Ok(provider_events_to_run_result(
            &events,
            format!("{provider}:{wire_api}:stream"),
            None,
        ));
    }
    let raw = response
        .text()
        .map_err(|error| format!("provider response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "provider returned HTTP {}: {}",
            status.as_u16(),
            summarize_http_error_body(&raw, &content_type)
        ));
    }
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("provider response was not JSON: {error}"))?;
    if wire_api == "chat" {
        let answer = extract_chat_answer(&value);
        let tool_calls = extract_chat_tool_calls(&value);
        let finish_reason = extract_chat_finish_reason(&value).unwrap_or_else(|| {
            if tool_calls.is_empty() {
                "stop"
            } else {
                "tool_call"
            }
            .to_string()
        });
        Ok(ProviderRunResult {
            answer: if answer.is_empty() && tool_calls.is_empty() {
                python_json_dumps(&value)
            } else {
                answer
            },
            tool_calls,
            usage: usage_from_json(value.get("usage")),
            source: format!("{provider}:{wire_api}"),
            finish_reason,
        })
    } else {
        let events = normalize_openai_responses_response(&value);
        Ok(provider_events_to_run_result(
            &events,
            format!("{provider}:{wire_api}"),
            Some(&value),
        ))
    }
}

fn call_anthropic_provider(
    args: &[String],
    api_key: &str,
    model_id: &str,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    mut stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
) -> Result<ProviderRunResult, String> {
    let timeout = Duration::from_secs(60);
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())?;
    let mut config = AnthropicLanguageModelConfig::new(api_key, model_id);
    config.base_url =
        value_for(args, &["--base-url"]).or_else(|| provider_env_value("anthropic", "base_url"));
    let stream = provider_streaming_enabled(args);
    let mut payload = build_anthropic_payload(&config, None, messages, tools, None, None, None);
    if let Some(object) = payload.as_object_mut() {
        object.insert("stream".to_string(), json!(stream));
    }
    let endpoint = join_url(
        config
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com/v1"),
        "messages",
    );
    let mut request = client
        .post(endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json");
    if stream {
        request = request.header("accept", "text/event-stream");
    }
    let response = request
        .json(&payload)
        .send()
        .map_err(|error| format!("anthropic request failed: {error}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if stream {
        if !status.is_success() {
            let raw = response
                .text()
                .map_err(|error| format!("anthropic response read failed: {error}"))?;
            return Err(format!(
                "anthropic returned HTTP {}: {}",
                status.as_u16(),
                summarize_http_error_body(&raw, &content_type)
            ));
        }
        let mut chunks = Vec::new();
        read_sse_json_values_stream(response, |chunk| {
            if let Some(event) = anthropic_stream_text_delta(&chunk)
                && let Some(sink) = stream_sink.as_deref_mut()
            {
                sink(&event);
            }
            chunks.push(chunk);
            Ok(())
        })?;
        let events = normalize_anthropic_events(&chunks);
        return Ok(provider_events_to_run_result(
            &events,
            "anthropic:messages:stream".to_string(),
            None,
        ));
    }
    let raw = response
        .text()
        .map_err(|error| format!("anthropic response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "anthropic returned HTTP {}: {}",
            status.as_u16(),
            summarize_http_error_body(&raw, &content_type)
        ));
    }
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("anthropic response was not JSON: {error}"))?;
    let answer = value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    let tool_calls = extract_anthropic_tool_calls(&value);
    Ok(ProviderRunResult {
        answer,
        tool_calls,
        usage: usage_from_json(value.get("usage")),
        source: "anthropic:messages".to_string(),
        finish_reason: value
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("stop")
            .to_string(),
    })
}

fn provider_events_to_run_result(
    events: &[ProviderStreamEvent],
    source: String,
    fallback_json: Option<&Value>,
) -> ProviderRunResult {
    let mut answer = String::new();
    let mut usage = Usage::default();
    let mut tool_calls = Vec::new();
    let mut finish_reason = "stop".to_string();
    for event in events {
        match event {
            ProviderStreamEvent::TextDelta { text } => answer.push_str(text),
            ProviderStreamEvent::Finish {
                usage: item,
                finish_reason: reason,
            } => {
                usage = item.clone();
                finish_reason = reason.clone();
            }
            ProviderStreamEvent::ToolCall {
                call_id,
                name,
                input,
            } => {
                tool_calls.push(ToolCall {
                    name: name.clone(),
                    input: input.clone(),
                    call_id: call_id.clone(),
                });
            }
        }
    }
    if answer.is_empty()
        && tool_calls.is_empty()
        && let Some(value) = fallback_json
    {
        answer = python_json_dumps(value);
    }
    ProviderRunResult {
        answer,
        tool_calls,
        usage,
        source,
        finish_reason,
    }
}

fn provider_streaming_enabled(args: &[String]) -> bool {
    has_flag(args, &["--stream"])
        || env::var("OPENAGENT_STREAM").is_ok_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

fn parse_sse_json_values(raw: &str) -> Result<Vec<Value>, String> {
    let mut values = Vec::new();
    let mut data_lines = Vec::new();
    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            flush_sse_json_value(&mut data_lines, &mut values)?;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }
    flush_sse_json_value(&mut data_lines, &mut values)?;
    Ok(values)
}

fn read_sse_json_values_stream<R, F>(mut reader: R, mut on_value: F) -> Result<(), String>
where
    R: Read,
    F: FnMut(Value) -> Result<(), String>,
{
    let mut raw = String::new();
    let mut buffer = [0_u8; 4096];
    let mut saw_done = false;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(_error) if saw_done => break,
            Err(error) => return Err(format!("provider SSE read failed: {error}")),
        };
        if read == 0 {
            break;
        }
        raw.push_str(&String::from_utf8_lossy(&buffer[..read]));
        while let Some(index) = sse_frame_end(&raw) {
            let frame = raw[..index].to_string();
            let drain_to = if raw[index..].starts_with("\r\n\r\n") {
                index + 4
            } else {
                index + 2
            };
            raw.drain(..drain_to);
            if sse_frame_is_done(&frame) {
                saw_done = true;
            }
            if let Some(value) = parse_sse_frame_json(&frame)? {
                on_value(value)?;
            }
        }
    }
    if !raw.trim().is_empty()
        && let Some(value) = parse_sse_frame_json(&raw)?
    {
        on_value(value)?;
    }
    Ok(())
}

fn sse_frame_is_done(frame: &str) -> bool {
    frame.lines().any(|line| {
        let line = line.trim_end_matches('\r');
        line.strip_prefix("data:")
            .map(str::trim)
            .is_some_and(|data| data == "[DONE]")
    })
}

fn sse_frame_end(raw: &str) -> Option<usize> {
    match (raw.find("\r\n\r\n"), raw.find("\n\n")) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(index), None) | (None, Some(index)) => Some(index),
        (None, None) => None,
    }
}

fn parse_sse_frame_json(frame: &str) -> Result<Option<Value>, String> {
    let mut data_lines = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }
    if data_lines.is_empty() {
        return Ok(None);
    }
    let data = data_lines.join("\n");
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .map_err(|error| format!("provider SSE data was not JSON: {error}"))
}

fn flush_sse_json_value(
    data_lines: &mut Vec<String>,
    values: &mut Vec<Value>,
) -> Result<(), String> {
    if data_lines.is_empty() {
        return Ok(());
    }
    let data = data_lines.join("\n");
    data_lines.clear();
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(());
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|error| format!("provider SSE data was not JSON: {error}"))?;
    values.push(value);
    Ok(())
}

fn openai_stream_text_delta(wire_api: &str, chunk: &Value) -> Option<ProviderStreamEvent> {
    let text = if wire_api == "chat" {
        chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("content"))
            .or_else(|| {
                chunk
                    .get("choices")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(|choice| choice.get("text"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else if matches!(
        chunk.get("type").and_then(Value::as_str),
        Some("response.output_text.delta" | "response.refusal.delta")
    ) {
        chunk
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else {
        ""
    };
    (!text.is_empty()).then(|| ProviderStreamEvent::TextDelta {
        text: text.to_string(),
    })
}

fn anthropic_stream_text_delta(chunk: &Value) -> Option<ProviderStreamEvent> {
    let text = match chunk
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "content_block_start" => chunk
            .get("content_block")
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|block| block.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "content_block_delta" => chunk
            .get("delta")
            .filter(|delta| delta.get("type").and_then(Value::as_str) == Some("text_delta"))
            .and_then(|delta| delta.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        _ => "",
    };
    (!text.is_empty()).then(|| ProviderStreamEvent::TextDelta {
        text: text.to_string(),
    })
}

fn filter_tools_for_agent(
    tools: Vec<ToolSchema>,
    agent_profile: Option<&RunAgentProfile>,
) -> Vec<ToolSchema> {
    let Some(profile) = agent_profile else {
        return tools;
    };
    if profile.tools.is_empty() {
        return tools;
    }
    tools
        .into_iter()
        .filter(|tool| {
            profile
                .tools
                .iter()
                .any(|pattern| wildcard_match(pattern, &tool.name))
        })
        .collect()
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" || pattern == value {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return value.ends_with(suffix);
    }
    if let Some((prefix, suffix)) = pattern.split_once('*') {
        return value.starts_with(prefix) && value.ends_with(suffix);
    }
    false
}

fn load_mcp_runtime(args: &[String], toolkit: &mut Toolkit) -> Result<Option<McpRuntime>, String> {
    let Some(source) = mcp_runtime_source(args) else {
        return Ok(None);
    };
    let config = load_mcp_config(&source)?;
    if !config.enabled() {
        return Ok(Some(McpRuntime {
            manager: RemoteMcpManager::new(config),
            descriptors: BTreeMap::new(),
            snapshot: json!({}),
        }));
    }
    let mut manager = RemoteMcpManager::new(config.clone());
    let mut descriptors_by_name = BTreeMap::new();
    for server in config.servers.iter().filter(|server| server.enabled) {
        let (transport, tools) = discover_mcp_server_tools(server)?;
        let descriptors = build_tool_descriptors_from_values(server, &tools);
        for descriptor in &descriptors {
            toolkit
                .registry
                .register(mcp_tool_definition(descriptor, "remote-mcp"));
            descriptors_by_name.insert(descriptor.dynamic_name.clone(), descriptor.clone());
        }
        manager.set_server_tools(
            &server.name,
            Some(transport),
            "connected",
            Some(now_ms_cli() as f64 / 1000.0),
            descriptors,
        )?;
    }
    let snapshot = serde_json::to_value(manager.snapshot()).unwrap_or_else(|_| json!({}));
    Ok(Some(McpRuntime {
        manager,
        descriptors: descriptors_by_name,
        snapshot,
    }))
}

fn mcp_runtime_source(args: &[String]) -> Option<String> {
    value_for(args, &["--mcp-config"])
        .or_else(|| env::var("OPENAGENT_MCP_CONFIG").ok())
        .or_else(|| {
            let path = mcp_config_path(args);
            path.exists().then(|| path.to_string_lossy().to_string())
        })
}

fn discover_mcp_server_tools(
    server: &RemoteMcpServerConfig,
) -> Result<(McpTransport, Vec<Value>), String> {
    let mut errors = Vec::new();
    for transport in transport_candidates(server.transport) {
        match mcp_json_rpc(server, transport, "tools/list", json!({})) {
            Ok(value) => {
                let tools = value
                    .get("tools")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                return Ok((transport, tools));
            }
            Err(error) => errors.push(format!("{}: {error}", transport.as_str())),
        }
    }
    Err(format!(
        "MCP tools/list failed for server '{}': {}",
        server.name,
        errors.join("; ")
    ))
}

fn mcp_json_rpc(
    server: &RemoteMcpServerConfig,
    transport: McpTransport,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let timeout = Duration::from_millis(server.timeout_ms);
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client
        .post(&server.url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": format!("openagent-{}", now_ms_cli()),
            "method": method,
            "params": params,
        }));
    for (key, value) in &server.headers {
        request = request.header(key, value);
    }
    let response = request
        .send()
        .map_err(|error| format!("{} request failed: {error}", transport.as_str()))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response
        .text()
        .map_err(|error| format!("MCP response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            summarize_http_error_body(&raw, &content_type)
        ));
    }
    let value = if content_type.contains("text/event-stream") {
        parse_sse_json_values(&raw)?
            .into_iter()
            .find(|item| item.get("result").is_some() || item.get("error").is_some())
            .ok_or_else(|| "MCP SSE response did not contain a JSON-RPC result".to_string())?
    } else {
        serde_json::from_str::<Value>(&raw)
            .map_err(|error| format!("MCP response was not JSON: {error}"))?
    };
    if let Some(error) = value.get("error") {
        return Err(format!("MCP JSON-RPC error: {}", python_json_dumps(error)));
    }
    Ok(value.get("result").cloned().unwrap_or(value))
}

fn execute_agent_tool(
    toolkit: &Toolkit,
    mcp_runtime: Option<&McpRuntime>,
    tool_call: &ToolCall,
    ctx: &mut ToolContext,
) -> ToolResult {
    if let Some(result) = execute_mcp_tool(mcp_runtime, tool_call) {
        return result;
    }
    toolkit.execute(
        &tool_call.name,
        tool_call.input.clone(),
        &tool_call.call_id,
        ctx,
    )
}

fn execute_mcp_tool(mcp_runtime: Option<&McpRuntime>, tool_call: &ToolCall) -> Option<ToolResult> {
    let runtime = mcp_runtime?;
    let descriptor = runtime.descriptors.get(&tool_call.name)?;
    let Some(state) = runtime.manager.servers.get(&descriptor.server_name) else {
        let result = unavailable_tool_result(&tool_call.name);
        let bridge = bridge_tool_output(descriptor, result);
        return Some(mcp_bridge_to_tool_result(tool_call, bridge));
    };
    let transport = state.selected_transport.unwrap_or(McpTransport::Http);
    let result = match mcp_json_rpc(
        &state.config,
        transport,
        "tools/call",
        json!({
            "name": descriptor.original_name,
            "arguments": tool_call.input.clone(),
        }),
    ) {
        Ok(value) => normalize_tool_call_result(descriptor, Some(transport), &value),
        Err(error) => {
            let mut result = unavailable_tool_result(&tool_call.name);
            result.error = Some(error);
            result
        }
    };
    Some(mcp_bridge_to_tool_result(
        tool_call,
        bridge_tool_output(descriptor, result),
    ))
}

fn mcp_bridge_to_tool_result(
    tool_call: &ToolCall,
    bridge: openagent_mcp::McpBridgeOutput,
) -> ToolResult {
    ToolResult {
        call_id: tool_call.call_id.clone(),
        output: bridge.output,
        error: bridge.error,
        metadata: bridge.metadata,
    }
}

fn pending_resume_from_session(session: &Session) -> Option<PendingResume> {
    if let Some(response) = session.metadata.get("pending_question_response")
        && let Some(pending) = session.metadata.get("pending_question")
    {
        return pending_resume_from_values("question", pending, response);
    }
    if let Some(response) = session.metadata.get("pending_approval_response")
        && let Some(pending) = session.metadata.get("pending_approval")
    {
        return pending_resume_from_values("approval", pending, response);
    }
    None
}

fn pending_resume_from_values(
    kind: &str,
    pending: &Value,
    response: &Value,
) -> Option<PendingResume> {
    let call_id = pending.get("call_id").and_then(Value::as_str)?.to_string();
    let tool_name = pending
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or(if kind == "question" { "question" } else { "" })
        .to_string();
    if tool_name.is_empty() {
        return None;
    }
    Some(PendingResume {
        kind: kind.to_string(),
        request_id: pending
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or(&call_id)
            .to_string(),
        call: ToolCall {
            name: tool_name,
            input: pending
                .get("tool_input")
                .or_else(|| pending.get("toolInput"))
                .cloned()
                .unwrap_or_else(|| json!({})),
            call_id,
        },
        response: response.clone(),
        step: pending.get("step").and_then(Value::as_u64).unwrap_or(0),
    })
}

struct PendingResumeContext<'a, 'sink> {
    toolkit: &'a Toolkit,
    mcp_runtime: Option<&'a McpRuntime>,
    ctx: &'a mut ToolContext,
    session: &'a mut Session,
    store: &'a FileSessionStore,
    run_id: &'a str,
    events: &'a mut Vec<Value>,
    event_sink: &'a mut Option<&'sink mut dyn FnMut(&Value)>,
}

fn process_pending_resume(
    pending: PendingResume,
    context: &mut PendingResumeContext<'_, '_>,
) -> Result<(), String> {
    emit_run_event(
        context.events,
        json!({
            "method": format!("turn/{}_resumed", pending.kind),
            "params": {
                "session_id": context.session.id.clone(),
                "run_id": context.run_id,
                "request_id": pending.request_id.clone(),
                "call_id": pending.call.call_id.clone(),
            }
        }),
        context.event_sink,
    );
    let result = if pending.kind == "question" {
        let answers = pending
            .response
            .get("answers")
            .and_then(question_answers_from_json)
            .or_else(|| {
                pending
                    .response
                    .get("answer")
                    .and_then(value_to_answer_string)
                    .map(|answer| vec![vec![answer]])
            })
            .unwrap_or_default();
        context.ctx.set_question_answers(answers);
        context.toolkit.execute(
            "question",
            pending.call.input.clone(),
            &pending.call.call_id,
            context.ctx,
        )
    } else {
        let decision = pending
            .response
            .get("decision")
            .and_then(Value::as_str)
            .unwrap_or("allow_once");
        if matches!(decision, "reject" | "deny") {
            ToolResult {
                call_id: pending.call.call_id.clone(),
                output: String::new(),
                error: Some(
                    pending
                        .response
                        .get("note")
                        .and_then(Value::as_str)
                        .unwrap_or("Permission rejected by user")
                        .to_string(),
                ),
                metadata: BTreeMap::from([
                    ("tool".to_string(), json!(pending.call.name.clone())),
                    ("permission_action".to_string(), json!("reject")),
                    ("request_id".to_string(), json!(pending.request_id.clone())),
                ]),
            }
        } else {
            if matches!(decision, "allow_always" | "always")
                && let Some(pattern) = context
                    .session
                    .metadata
                    .get("pending_approval")
                    .and_then(|item| item.get("permission_pattern"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            {
                add_approval_always_pattern(context.session, pattern);
            }
            let previous = context.ctx.dangerously_skip_permissions;
            context.ctx.dangerously_skip_permissions = true;
            let result = execute_agent_tool(
                context.toolkit,
                context.mcp_runtime,
                &pending.call,
                context.ctx,
            );
            context.ctx.dangerously_skip_permissions = previous;
            result
        }
    };
    append_tool_result_to_session(context, pending.step, &pending.call, result)?;
    context.session.metadata.remove("pending_question");
    context.session.metadata.remove("pending_question_response");
    context.session.metadata.remove("pending_approval");
    context.session.metadata.remove("pending_approval_response");
    context
        .store
        .save_state(context.session, Some(context.run_id))
        .map_err(|error| format!("failed to save resumed session state: {error}"))?;
    Ok(())
}

fn append_tool_result_to_session(
    context: &mut PendingResumeContext<'_, '_>,
    step: u64,
    tool_call: &ToolCall,
    tool_result: ToolResult,
) -> Result<(), String> {
    let failed = tool_result.error.is_some();
    emit_run_event(
        context.events,
        json!({
            "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
            "params": {
                "session_id": context.session.id.clone(),
                "run_id": context.run_id,
                "step": step,
                "call_id": tool_call.call_id.clone(),
                "name": tool_call.name.clone(),
                "output": tool_result.output.clone(),
                "error": tool_result.error.clone(),
                "metadata": tool_result.metadata.clone(),
            }
        }),
        context.event_sink,
    );
    let _ = context.store.record_event(
        &context.session.id,
        context.run_id,
        if failed {
            "tool.call.failed"
        } else {
            "tool.call.finished"
        },
        SessionEventOptions {
            kind: "tool".to_string(),
            status: if failed {
                "error".to_string()
            } else {
                "ok".to_string()
            },
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("error".to_string(), json!(tool_result.error.clone())),
                ("metadata".to_string(), json!(tool_result.metadata.clone())),
                ("step".to_string(), json!(step)),
            ]),
            ..SessionEventOptions::default()
        },
    );
    let _ = context.store.append_part(
        &context.session.id,
        context.run_id,
        "tool_result",
        SessionPartOptions {
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("failed".to_string(), json!(failed)),
            ]),
            step_index: Some(step),
            ..SessionPartOptions::default()
        },
    );
    let mut tool_message = chat_message(
        Role::Tool,
        tool_result.error.as_ref().map_or_else(
            || tool_result.output.clone(),
            |error| format!("Tool failed: {error}"),
        ),
    );
    tool_message.name = Some(tool_call.name.clone());
    tool_message.tool_call_id = Some(tool_call.call_id.clone());
    tool_message
        .metadata
        .insert("tool_result".to_string(), json!(tool_result));
    if let Some(message_id) = context
        .session
        .metadata
        .get(if tool_call.name == "question" {
            "pending_question"
        } else {
            "pending_approval"
        })
        .and_then(|value| value.get("assistant_message_id"))
        .and_then(Value::as_str)
    {
        tool_message
            .metadata
            .insert("assistant_message_id".to_string(), json!(message_id));
    }
    tool_message
        .metadata
        .insert("step".to_string(), json!(step));
    let tool_index = context.session.messages.len() as u64;
    context.session.add(tool_message.clone());
    context
        .store
        .append_message(context.session, &tool_message, context.run_id, tool_index)
        .map_err(|error| format!("failed to record resumed tool message: {error}"))
}

fn approval_always_patterns(session: &Session) -> Vec<String> {
    session
        .metadata
        .get("approval_always_patterns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn add_approval_always_pattern(session: &mut Session, pattern: String) {
    if pattern.is_empty() {
        return;
    }
    let mut patterns = approval_always_patterns(session);
    if !patterns.iter().any(|item| item == &pattern) {
        patterns.push(pattern);
    }
    session
        .metadata
        .insert("approval_always_patterns".to_string(), json!(patterns));
}

fn normalize_openai_responses_stream_events(chunks: &[Value]) -> Vec<ProviderStreamEvent> {
    let mut events = Vec::new();
    let mut finish_reason = "stop".to_string();
    let mut usage = Usage::default();
    for chunk in chunks {
        let event_type = chunk
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "response.output_text.delta" | "response.refusal.delta" => {
                let text = chunk
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !text.is_empty() {
                    events.push(ProviderStreamEvent::TextDelta {
                        text: text.to_string(),
                    });
                }
            }
            "response.output_item.done" => {
                if let Some(tool_call) =
                    response_stream_tool_call(chunk.get("item").unwrap_or(&Value::Null))
                {
                    finish_reason = "tool_call".to_string();
                    events.push(tool_call);
                }
            }
            "response.completed" => {
                if let Some(response) = chunk.get("response") {
                    let nested = normalize_openai_responses_response(response);
                    if !nested.is_empty() {
                        usage = nested
                            .iter()
                            .find_map(|event| match event {
                                ProviderStreamEvent::Finish { usage, .. } => Some(usage.clone()),
                                _ => None,
                            })
                            .unwrap_or(usage);
                    }
                }
            }
            "response.failed" | "response.incomplete" => {
                finish_reason = "error".to_string();
            }
            _ => {}
        }
    }
    events.push(ProviderStreamEvent::Finish {
        finish_reason,
        usage,
    });
    events
}

fn response_stream_tool_call(item: &Value) -> Option<ProviderStreamEvent> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    if !matches!(item_type, "function_call" | "custom_tool_call") {
        return None;
    }
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("responses_tool_call")
        .to_string();
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let input = item
        .get("arguments")
        .or_else(|| item.get("input"))
        .map(parse_tool_arguments)
        .unwrap_or_else(|| json!({}));
    Some(ProviderStreamEvent::ToolCall {
        call_id,
        name,
        input,
    })
}

fn extract_chat_answer(value: &Value) -> String {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn extract_chat_tool_calls(value: &Value) -> Vec<ToolCall> {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("message"))
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, call)| {
            let function = call.get("function");
            let name = function
                .and_then(|item| item.get("name"))
                .or_else(|| call.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            let call_id = call
                .get("id")
                .or_else(|| call.get("call_id"))
                .and_then(Value::as_str)
                .map_or_else(|| format!("chat_call_{index}"), str::to_string);
            let arguments = function
                .and_then(|item| item.get("arguments"))
                .or_else(|| call.get("arguments"))
                .or_else(|| call.get("input"))
                .unwrap_or(&Value::Null);
            Some(ToolCall {
                name: name.to_string(),
                input: parse_tool_arguments(arguments),
                call_id,
            })
        })
        .collect()
}

fn extract_chat_finish_reason(value: &Value) -> Option<String> {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("finish_reason"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn extract_anthropic_tool_calls(value: &Value) -> Vec<ToolCall> {
    value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, item)| {
            if item.get("type").and_then(Value::as_str) != Some("tool_use") {
                return None;
            }
            let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            Some(ToolCall {
                name: name.to_string(),
                input: item.get("input").cloned().unwrap_or_else(|| json!({})),
                call_id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("toolu_{index}"), str::to_string),
            })
        })
        .collect()
}

fn usage_from_json(value: Option<&Value>) -> Usage {
    let Some(value) = value else {
        return Usage::default();
    };
    Usage {
        input_tokens: value
            .get("input_tokens")
            .or_else(|| value.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        output_tokens: value
            .get("output_tokens")
            .or_else(|| value.get("completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        cost: 0.0,
    }
}

fn add_usage(total: &mut Usage, item: &Usage) {
    total.input_tokens += item.input_tokens;
    total.output_tokens += item.output_tokens;
    total.cost += item.cost;
}

fn emit_run_event(
    events: &mut Vec<Value>,
    event: Value,
    event_sink: &mut Option<&mut dyn FnMut(&Value)>,
) {
    if let Some(emit) = event_sink.as_deref_mut() {
        emit(&event);
    }
    events.push(event);
}

fn assistant_message_for_provider_step(content: String, tool_calls: &[ToolCall]) -> ChatMessage {
    let mut message = chat_message(Role::Assistant, content);
    if !tool_calls.is_empty() {
        message.metadata.insert(
            "tool_calls".to_string(),
            Value::Array(tool_calls.iter().map(openai_tool_call_value).collect()),
        );
    }
    message
}

fn openai_tool_call_value(call: &ToolCall) -> Value {
    json!({
        "id": call.call_id.clone(),
        "call_id": call.call_id.clone(),
        "type": "function",
        "function": {
            "name": call.name.clone(),
            "arguments": python_json_dumps(&call.input),
        },
        "name": call.name.clone(),
        "input": call.input.clone(),
    })
}

fn approval_payload_for_tool_call(
    session: &Session,
    run_id: &str,
    step: u64,
    call: &ToolCall,
    metadata: &BTreeMap<String, Value>,
) -> Value {
    json!({
        "request_id": format!("approval_{}", call.call_id),
        "session_id": session.id.clone(),
        "turn_id": run_id,
        "run_id": run_id,
        "step": step,
        "tool_name": call.name.clone(),
        "tool_input": call.input.clone(),
        "call_id": call.call_id.clone(),
        "created_at_ms": now_ms_cli(),
        "permission_action": metadata.get("permission_action").cloned().unwrap_or_else(|| json!("ask")),
        "permission_pattern": metadata.get("permission_pattern").cloned().unwrap_or_else(|| json!("")),
        "reason": metadata.get("error_kind").cloned().unwrap_or_else(|| json!("permission_required")),
        "metadata": metadata,
    })
}

fn permission_ruleset_from_args(
    args: &[String],
    agent_profile: Option<&RunAgentProfile>,
) -> Result<PermissionRuleset, String> {
    let raw = value_for(args, &["--permission"])
        .or_else(|| agent_profile.and_then(|profile| profile.permission.clone()))
        .unwrap_or_else(|| "PLAN_ONLY".to_string());
    parse_permission_ruleset(&raw)
}

fn parse_permission_ruleset(raw: &str) -> Result<PermissionRuleset, String> {
    match raw.trim().to_ascii_uppercase().replace('-', "_").as_str() {
        "FULL" | "ALLOW" | "AUTO" => Ok(PermissionRuleset::Full),
        "READONLY" | "READ_ONLY" => Ok(PermissionRuleset::Readonly),
        "PLAN_ONLY" | "ASK" => Ok(PermissionRuleset::PlanOnly),
        "NONE" | "DENY" => Ok(PermissionRuleset::None),
        _ => Err("permission must be FULL, READONLY, PLAN_ONLY, or NONE".to_string()),
    }
}

fn configured_question_answers(args: &[String]) -> Option<Vec<Vec<String>>> {
    let cli_answers = values_for(args, &["--answer"])
        .into_iter()
        .map(|answer| split_answer_items(&answer))
        .collect::<Vec<_>>();
    if !cli_answers.is_empty() {
        return Some(cli_answers);
    }
    let raw = env::var("OPENAGENT_QUESTION_ANSWERS")
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    if let Ok(value) = serde_json::from_str::<Value>(&raw) {
        if let Some(parsed) = question_answers_from_json(&value) {
            return Some(parsed);
        }
    }
    Some(
        raw.split(';')
            .filter(|item| !item.trim().is_empty())
            .map(split_answer_items)
            .collect(),
    )
}

fn question_answers_from_json(value: &Value) -> Option<Vec<Vec<String>>> {
    let items = value.as_array()?;
    if items.iter().all(Value::is_array) {
        return Some(
            items
                .iter()
                .map(|item| {
                    item.as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(value_to_answer_string)
                        .collect::<Vec<_>>()
                })
                .collect(),
        );
    }
    Some(
        items
            .iter()
            .filter_map(value_to_answer_string)
            .map(|answer| vec![answer])
            .collect(),
    )
}

fn value_to_answer_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_bool().map(|item| item.to_string()))
        .or_else(|| value.as_i64().map(|item| item.to_string()))
        .or_else(|| value.as_u64().map(|item| item.to_string()))
        .or_else(|| value.as_f64().map(|item| item.to_string()))
}

fn split_answer_items(answer: &str) -> Vec<String> {
    answer
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn mock_tool_calls_from_env() -> Result<Option<Vec<ToolCall>>, String> {
    let Some(raw) = env::var("OPENAGENT_MOCK_TOOL_CALLS")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("OPENAGENT_MOCK_TOOL_CALLS is not JSON: {error}"))?;
    let items = if let Some(items) = value.as_array() {
        items.clone()
    } else {
        vec![value]
    };
    let mut calls = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "mock tool call requires name".to_string())?;
        let call_id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map_or_else(|| format!("mock_call_{index}"), str::to_string);
        let input = item
            .get("input")
            .or_else(|| item.get("arguments"))
            .map(parse_tool_arguments)
            .unwrap_or_else(|| json!({}));
        calls.push(ToolCall {
            name: name.to_string(),
            input,
            call_id,
        });
    }
    Ok(Some(calls))
}

fn record_step_finished(
    store: &FileSessionStore,
    session_id: &str,
    run_id: &str,
    step: u64,
    finish_reason: &str,
    tool_calls: u64,
    usage: &Usage,
) {
    let _ = store.record_event(
        session_id,
        run_id,
        "step.finished",
        SessionEventOptions {
            kind: "step".to_string(),
            attributes: BTreeMap::from([
                ("step".to_string(), json!(step)),
                ("finish_reason".to_string(), json!(finish_reason)),
                ("tool_calls".to_string(), json!(tool_calls)),
                ("input_tokens".to_string(), json!(usage.input_tokens)),
                ("output_tokens".to_string(), json!(usage.output_tokens)),
            ]),
            ..SessionEventOptions::default()
        },
    );
}

fn provider_api_key(provider: &str, args: &[String]) -> Option<String> {
    value_for(args, &["--api-key"]).or_else(|| provider_env_value(provider, "api_key"))
}

fn provider_base_url(provider: &str, args: &[String]) -> String {
    value_for(args, &["--base-url"])
        .or_else(|| provider_env_value(provider, "base_url"))
        .or_else(|| provider_default_base_url(provider).ok().flatten())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn is_synthetic_endpoint(base_url: &str) -> bool {
    base_url.contains(".test")
        || base_url.contains("example.com")
        || base_url.contains("example/v1")
        || base_url.contains("localhost:0")
}

fn provider_wire_api(provider: &str, args: &[String]) -> String {
    value_for(args, &["--wire-api"])
        .or_else(|| provider_env_value(provider, "wire_api"))
        .unwrap_or_else(|| {
            if provider == "anthropic" {
                "messages".to_string()
            } else {
                DEFAULT_WIRE_API.to_string()
            }
        })
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

fn models_cache_path() -> PathBuf {
    env::var("OPENAGENT_MODELS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".cache/openagent/models.json"))
}

#[derive(Clone, Debug)]
struct ModelsCacheLoad {
    value: Value,
    path: PathBuf,
    snapshot_path: PathBuf,
    status: String,
    refreshed: bool,
    stale: bool,
    fallback: bool,
    error: Option<String>,
}

impl ModelsCacheLoad {
    fn to_value(&self) -> Value {
        json!({
            "path": self.path.to_string_lossy(),
            "snapshot_path": self.snapshot_path.to_string_lossy(),
            "status": self.status,
            "refreshed": self.refreshed,
            "stale": self.stale,
            "fallback": self.fallback,
            "error": self.error,
            "source": self.value.get("source").cloned().unwrap_or(Value::Null),
        })
    }
}

struct ModelsCacheLock {
    path: PathBuf,
}

impl ModelsCacheLock {
    fn acquire(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if model_cache_lock_is_stale(&path) {
            let _ = fs::remove_file(&path);
        }
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = writeln!(
                    file,
                    "pid={} created_at_ms={}",
                    std::process::id(),
                    now_ms_cli()
                );
                Ok(Self { path })
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(format!(
                "models cache refresh locked: {}",
                path.to_string_lossy()
            )),
            Err(error) => Err(format!("failed to lock models cache: {error}")),
        }
    }
}

impl Drop for ModelsCacheLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn ensure_models_cache(args: &[String]) -> ModelsCacheLoad {
    let path = models_cache_path();
    let snapshot_path = models_cache_snapshot_path();
    let force_refresh = has_flag(args, &["--refresh"]);
    let offline = has_flag(args, &["--offline"]);
    let ttl_seconds = models_cache_ttl_seconds(args);
    let current = load_models_cache_file(&path, ttl_seconds);
    let current_stale = current
        .as_ref()
        .is_none_or(|value| models_cache_is_stale(value, ttl_seconds));
    let should_refresh = force_refresh || (current.is_some() && current_stale);
    if !offline && should_refresh {
        match refresh_models_cache(args) {
            Ok(value) => {
                let stale = models_cache_is_stale(&value, ttl_seconds);
                return ModelsCacheLoad {
                    value,
                    path,
                    snapshot_path,
                    status: "refreshed".to_string(),
                    refreshed: true,
                    stale,
                    fallback: false,
                    error: None,
                };
            }
            Err(error) => {
                if let Some(value) = current {
                    return ModelsCacheLoad {
                        value,
                        path,
                        snapshot_path,
                        status: "stale_refresh_failed".to_string(),
                        refreshed: false,
                        stale: true,
                        fallback: true,
                        error: Some(error),
                    };
                }
                if let Some(value) = load_models_cache_file(&snapshot_path, ttl_seconds) {
                    return ModelsCacheLoad {
                        value,
                        path,
                        snapshot_path,
                        status: "snapshot_fallback".to_string(),
                        refreshed: false,
                        stale: true,
                        fallback: true,
                        error: Some(error),
                    };
                }
                return ModelsCacheLoad {
                    value: empty_models_cache(ttl_seconds),
                    path,
                    snapshot_path,
                    status: "empty_refresh_failed".to_string(),
                    refreshed: false,
                    stale: true,
                    fallback: true,
                    error: Some(error),
                };
            }
        }
    }
    if let Some(value) = current {
        return ModelsCacheLoad {
            value,
            path,
            snapshot_path,
            status: if current_stale { "stale" } else { "hit" }.to_string(),
            refreshed: false,
            stale: current_stale,
            fallback: false,
            error: None,
        };
    }
    if let Some(value) = load_models_cache_file(&snapshot_path, ttl_seconds) {
        return ModelsCacheLoad {
            value,
            path,
            snapshot_path,
            status: "snapshot_fallback".to_string(),
            refreshed: false,
            stale: true,
            fallback: true,
            error: None,
        };
    }
    ModelsCacheLoad {
        value: empty_models_cache(ttl_seconds),
        path,
        snapshot_path,
        status: "empty".to_string(),
        refreshed: false,
        stale: true,
        fallback: true,
        error: None,
    }
}

fn refresh_models_cache(args: &[String]) -> Result<Value, String> {
    let path = models_cache_path();
    let _lock = ModelsCacheLock::acquire(models_cache_lock_path())?;
    let url = models_source_url(args);
    let endpoint = join_url(&url, "api.json");
    let raw = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(
            value_for(args, &["--timeout-s"])
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(20),
        ))
        .build()
        .map_err(|error| error.to_string())?
        .get(&endpoint)
        .send()
        .map_err(|error| format!("failed to fetch models cache: {error}"))?
        .text()
        .map_err(|error| format!("failed to read models cache: {error}"))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("models cache response was not JSON: {error}"))?;
    let normalized = normalize_models_catalog(&value, &endpoint, models_cache_ttl_seconds(args));
    write_json_file(&path, &normalized)?;
    write_json_file(&models_cache_snapshot_path(), &normalized)?;
    Ok(normalized)
}

fn load_models_cache_file(path: &Path, ttl_seconds: u64) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    if value
        .get("schema_version")
        .and_then(Value::as_str)
        .is_some_and(|schema| schema == "openagent.models_cache.v1")
    {
        return Some(value);
    }
    Some(normalize_models_catalog(&value, "local-cache", ttl_seconds))
}

fn empty_models_cache(ttl_seconds: u64) -> Value {
    json!({
        "schema_version": "openagent.models_cache.v1",
        "source": {
            "url": null,
            "fetched_at_ms": 0,
            "ttl_seconds": ttl_seconds,
            "provider_count": 0,
            "model_count": 0,
            "raw_schema": "empty",
        },
        "providers": {},
        "catalog": [],
    })
}

fn normalize_models_catalog(value: &Value, source_url: &str, ttl_seconds: u64) -> Value {
    if value
        .get("schema_version")
        .and_then(Value::as_str)
        .is_some_and(|schema| schema == "openagent.models_cache.v1")
    {
        return value.clone();
    }
    let mut providers = Map::new();
    let mut catalog = Vec::new();
    let mut model_count = 0_usize;
    let Some(source_providers) = value.as_object() else {
        return empty_models_cache(ttl_seconds);
    };
    for (raw_provider_id, provider_value) in source_providers {
        let Some(provider_object) = provider_value.as_object() else {
            continue;
        };
        let provider_id = normalize_models_provider_id(raw_provider_id);
        let upstream_id = provider_object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(raw_provider_id);
        let models = provider_object
            .get("models")
            .and_then(Value::as_object)
            .map(|items| {
                items
                    .iter()
                    .map(|(model_id, model)| {
                        normalize_models_dev_model(&provider_id, model_id, model)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let provider_model_count = models.len();
        model_count += provider_model_count;
        let label = provider_label(&provider_id).unwrap_or_else(|_| {
            provider_object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&provider_id)
                .to_string()
        });
        let record = json!({
            "id": provider_id,
            "source_id": upstream_id,
            "name": provider_object.get("name").and_then(Value::as_str).unwrap_or(label.as_str()),
            "label": label,
            "api": provider_object.get("api").cloned().unwrap_or(Value::Null),
            "doc": provider_object.get("doc").cloned().unwrap_or(Value::Null),
            "npm": provider_object.get("npm").cloned().unwrap_or(Value::Null),
            "env": provider_object.get("env").cloned().unwrap_or_else(|| json!([])),
            "default_model": provider_default_model(&provider_id).ok().flatten(),
            "default_base_url": provider_default_base_url(&provider_id).ok().flatten(),
            "requires_api_key": provider_requires_api_key(&provider_id).unwrap_or(true),
            "native_streaming": provider_native_streaming(&provider_id),
            "model_count": provider_model_count,
            "models": models,
        });
        catalog.push(provider_catalog_summary(&record));
        providers.insert(provider_id, record);
    }
    catalog.sort_by(|left, right| {
        left.get("id")
            .and_then(Value::as_str)
            .cmp(&right.get("id").and_then(Value::as_str))
    });
    json!({
        "schema_version": "openagent.models_cache.v1",
        "source": {
            "url": source_url,
            "fetched_at_ms": now_ms_cli(),
            "ttl_seconds": ttl_seconds,
            "provider_count": providers.len(),
            "model_count": model_count,
            "raw_schema": "models.dev/api.json",
        },
        "providers": providers,
        "catalog": catalog,
    })
}

fn normalize_models_dev_model(provider_id: &str, model_id: &str, model: &Value) -> Value {
    let id = model
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(model_id)
        .to_string();
    let provider_model_id = strip_provider_prefix(provider_id, &id);
    let context_window = model
        .get("limit")
        .and_then(|limit| limit.get("context").or_else(|| limit.get("input")))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| openai_compatible_model(provider_id, &provider_model_id).context_window);
    let max_output = model
        .get("limit")
        .and_then(|limit| limit.get("output"))
        .and_then(Value::as_u64)
        .unwrap_or(4096);
    let input_modalities = model
        .get("modalities")
        .and_then(|modalities| modalities.get("input"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let vision = input_modalities.iter().any(|item| {
        item.as_str()
            .is_some_and(|value| matches!(value, "image" | "video" | "pdf"))
    }) || model
        .get("attachment")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    json!({
        "id": id,
        "provider_model_id": provider_model_id,
        "provider_id": provider_id,
        "name": model.get("name").and_then(Value::as_str).unwrap_or(model_id),
        "family": model.get("family").cloned().unwrap_or(Value::Null),
        "context_window": context_window,
        "max_output": max_output,
        "capabilities": {
            "vision": vision,
            "tools": model.get("tool_call").and_then(Value::as_bool).unwrap_or(true),
            "streaming": true,
            "reasoning": model.get("reasoning").and_then(Value::as_bool).unwrap_or(false),
            "structured_output": model.get("structured_output").and_then(Value::as_bool).unwrap_or(false),
            "temperature": model.get("temperature").and_then(Value::as_bool).unwrap_or(true),
        },
        "pricing": {
            "input_per_1m": model.get("cost").and_then(|cost| cost.get("input")).and_then(Value::as_f64).unwrap_or(0.0),
            "output_per_1m": model.get("cost").and_then(|cost| cost.get("output")).and_then(Value::as_f64).unwrap_or(0.0),
            "cache_read_per_1m": model.get("cost").and_then(|cost| cost.get("cache_read")).and_then(Value::as_f64).unwrap_or(0.0),
            "cache_write_per_1m": model.get("cost").and_then(|cost| cost.get("cache_write")).and_then(Value::as_f64).unwrap_or(0.0),
        },
        "modalities": model.get("modalities").cloned().unwrap_or(Value::Null),
        "knowledge": model.get("knowledge").cloned().unwrap_or(Value::Null),
        "release_date": model.get("release_date").cloned().unwrap_or(Value::Null),
        "last_updated": model.get("last_updated").cloned().unwrap_or(Value::Null),
        "open_weights": model.get("open_weights").cloned().unwrap_or(Value::Null),
        "raw": model,
    })
}

fn load_cached_provider_models_from_cache(cache: &Value, provider: &str) -> Option<Vec<Value>> {
    for key in provider_lookup_keys(provider) {
        if let Some(models) = cache
            .get("providers")
            .and_then(|providers| providers.get(&key))
            .and_then(|provider| provider.get("models"))
            .and_then(Value::as_array)
        {
            return Some(models.clone());
        }
        if let Some(models) = cache
            .get(&key)
            .and_then(|provider| provider.get("models"))
            .and_then(Value::as_object)
        {
            return Some(
                models
                    .iter()
                    .map(|(id, model)| {
                        let mut value = model.clone();
                        if let Some(object) = value.as_object_mut() {
                            object.entry("id".to_string()).or_insert_with(|| json!(id));
                        }
                        value
                    })
                    .collect(),
            );
        }
    }
    None
}

fn provider_catalog_record(cache: &Value, provider: &str) -> Option<Value> {
    provider_lookup_keys(provider).into_iter().find_map(|key| {
        cache
            .get("providers")
            .and_then(|providers| providers.get(&key))
            .cloned()
            .map(|record| {
                let mut summary = provider_catalog_summary(&record);
                if let Some(object) = summary.as_object_mut() {
                    object.insert("selected".to_string(), json!(key == provider));
                }
                summary
            })
    })
}

fn fallback_provider_catalog_record(provider: &str, model_count: usize) -> Value {
    json!({
        "id": provider,
        "label": provider_label(provider).unwrap_or_else(|_| provider.to_string()),
        "name": provider_label(provider).unwrap_or_else(|_| provider.to_string()),
        "default_model": provider_default_model(provider).ok().flatten(),
        "default_base_url": provider_default_base_url(provider).ok().flatten(),
        "requires_api_key": provider_requires_api_key(provider).unwrap_or(true),
        "native_streaming": provider_native_streaming(provider),
        "model_count": model_count,
        "source": "fallback",
    })
}

fn models_catalog_payload(cache: &ModelsCacheLoad) -> Value {
    json!({
        "schema_version": "openagent.models_catalog.v1",
        "cache": cache.to_value(),
        "providers": cache.value.get("catalog").cloned().unwrap_or_else(|| json!([])),
    })
}

fn models_catalog_text(payload: &Value, verbose: bool) -> String {
    let mut lines = vec![format!(
        "providers: {}",
        payload
            .get("providers")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    )];
    for provider in payload
        .get("providers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = provider.get("id").and_then(Value::as_str).unwrap_or("-");
        let label = provider.get("label").and_then(Value::as_str).unwrap_or(id);
        let count = provider
            .get("model_count")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if verbose {
            lines.push(format!(
                "{id} ({label}) models={count} {}",
                python_json_dumps(provider)
            ));
        } else {
            lines.push(format!("{id} ({label}) models={count}"));
        }
    }
    lines.join("\n")
}

fn provider_catalog_summary(provider: &Value) -> Value {
    json!({
        "id": provider.get("id").cloned().unwrap_or(Value::Null),
        "source_id": provider.get("source_id").cloned().unwrap_or(Value::Null),
        "name": provider.get("name").cloned().unwrap_or(Value::Null),
        "label": provider.get("label").cloned().unwrap_or(Value::Null),
        "api": provider.get("api").cloned().unwrap_or(Value::Null),
        "doc": provider.get("doc").cloned().unwrap_or(Value::Null),
        "npm": provider.get("npm").cloned().unwrap_or(Value::Null),
        "env": provider.get("env").cloned().unwrap_or_else(|| json!([])),
        "default_model": provider.get("default_model").cloned().unwrap_or(Value::Null),
        "default_base_url": provider.get("default_base_url").cloned().unwrap_or(Value::Null),
        "requires_api_key": provider.get("requires_api_key").cloned().unwrap_or(Value::Bool(true)),
        "native_streaming": provider.get("native_streaming").cloned().unwrap_or_else(|| json!({})),
        "model_count": provider.get("model_count").cloned().unwrap_or_else(|| json!(0)),
    })
}

fn provider_lookup_keys(provider: &str) -> Vec<String> {
    let normalized = normalize_models_provider_id(provider);
    let mut keys = vec![normalized.clone()];
    if normalized == "gemini" {
        keys.push("google".to_string());
    }
    keys.dedup();
    keys
}

fn normalize_models_provider_id(provider: &str) -> String {
    match provider {
        "google" => "gemini".to_string(),
        other => normalize_provider(Some(other)).unwrap_or_else(|_| other.to_string()),
    }
}

fn strip_provider_prefix(provider: &str, model_id: &str) -> String {
    for prefix in provider_lookup_keys(provider) {
        if let Some(value) = model_id.strip_prefix(&format!("{prefix}/")) {
            return value.to_string();
        }
    }
    if provider == "gemini"
        && let Some(value) = model_id.strip_prefix("google/")
    {
        return value.to_string();
    }
    model_id.to_string()
}

fn provider_native_streaming(provider: &str) -> Value {
    json!({
        "chat_completions_sse": provider != "anthropic",
        "responses_sse": provider != "anthropic",
        "anthropic_messages_sse": provider == "anthropic",
        "implemented": matches!(provider, "anthropic" | "openai" | "openrouter" | "groq" | "mistral" | "deepseek" | "xai" | "ollama" | "gemini" | "azure-openai"),
    })
}

fn compact_capabilities(capabilities: &Value) -> String {
    let mut parts = Vec::new();
    for key in [
        "vision",
        "tools",
        "streaming",
        "reasoning",
        "structured_output",
    ] {
        if capabilities
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            parts.push(key);
        }
    }
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(",")
    }
}

fn models_cache_snapshot_path() -> PathBuf {
    models_cache_path().with_extension("snapshot.json")
}

fn models_cache_lock_path() -> PathBuf {
    models_cache_path().with_extension("lock")
}

fn models_source_url(args: &[String]) -> String {
    value_for(args, &["--models-url"])
        .or_else(|| env::var("OPENAGENT_MODELS_URL").ok())
        .unwrap_or_else(|| "https://models.dev".to_string())
}

fn models_cache_ttl_seconds(args: &[String]) -> u64 {
    value_for(args, &["--ttl-seconds"])
        .or_else(|| env::var("OPENAGENT_MODELS_TTL_SECONDS").ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(24 * 60 * 60)
}

fn models_cache_is_stale(cache: &Value, ttl_seconds: u64) -> bool {
    let fetched_at = cache
        .get("source")
        .and_then(|source| source.get("fetched_at_ms"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if fetched_at == 0 {
        return true;
    }
    now_ms_cli().saturating_sub(fetched_at) > ttl_seconds.saturating_mul(1000)
}

fn model_cache_lock_is_stale(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|duration| duration > Duration::from_secs(120))
        .unwrap_or(false)
}

fn remote_select_session(
    server_url: &str,
    token: Option<&str>,
    explicit: Option<String>,
    continue_last: bool,
    workspace: &Path,
) -> Result<String, String> {
    if let Some(session_id) = explicit {
        return Ok(session_id);
    }
    if continue_last {
        let payload = http_json("GET", server_url, "/api/sessions", token, None)?;
        if let Some(session_id) = payload
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("session_id").or_else(|| item.get("id")))
            .and_then(Value::as_str)
        {
            return Ok(session_id.to_string());
        }
    }
    let payload = http_json(
        "POST",
        server_url,
        "/api/sessions",
        token,
        Some(json!({"cwd": workspace.to_string_lossy()})),
    )?;
    payload
        .get("session_id")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "server did not return a session id".to_string())
}

fn remote_select_session_with_auth(
    server_url: &str,
    auth: &RemoteAuth,
    explicit: Option<String>,
    continue_last: bool,
    fork: bool,
    workspace: &Path,
) -> Result<String, String> {
    if fork && explicit.is_none() && !continue_last {
        return Err("--fork requires --continue or --session".to_string());
    }
    let base = if let Some(session_id) = explicit {
        Some(session_id)
    } else if continue_last {
        let payload = http_json_with_auth("GET", server_url, "/api/sessions", auth, None)?;
        payload
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("session_id").or_else(|| item.get("id")))
            .and_then(Value::as_str)
            .map(str::to_string)
    } else {
        None
    };
    if !fork && let Some(session_id) = base {
        return Ok(session_id);
    }
    let mut body = json!({"cwd": workspace.to_string_lossy()});
    if let Some(fork_from) = base {
        body["fork_from"] = json!(fork_from);
    }
    let payload = http_json_with_auth("POST", server_url, "/api/sessions", auth, Some(body))?;
    payload
        .get("session_id")
        .or_else(|| payload.get("id"))
        .or_else(|| payload.get("session").and_then(|session| session.get("id")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "server did not return a session id".to_string())
}

fn remote_start_turn(
    server_url: &str,
    token: Option<&str>,
    session_id: &str,
    prompt: &str,
) -> Result<Value, String> {
    http_json(
        "POST",
        server_url,
        &format!("/api/sessions/{session_id}/turns"),
        token,
        Some(json!({"input": prompt})),
    )
}

fn remote_start_turn_with_auth(
    server_url: &str,
    auth: &RemoteAuth,
    session_id: &str,
    prompt: &str,
) -> Result<Value, String> {
    http_json_with_auth(
        "POST",
        server_url,
        &format!("/api/sessions/{session_id}/turns"),
        auth,
        Some(json!({"input": prompt})),
    )
}

fn remote_turn_events(
    server_url: &str,
    auth: &RemoteAuth,
    turn_id: &str,
    last_event_id: u64,
) -> Result<Vec<Value>, String> {
    let path = if last_event_id > 0 {
        format!("/api/turns/{turn_id}/events?last_event_id={last_event_id}")
    } else {
        format!("/api/turns/{turn_id}/events")
    };
    let raw = http_text_with_auth("GET", server_url, &path, auth, None)?;
    openagent_http_runtime::parse_sse_response_lines(&raw.lines().collect::<Vec<_>>())
}

fn remote_events_for_payload(
    server_url: &str,
    auth: &RemoteAuth,
    payload: &Value,
) -> Result<Vec<Value>, String> {
    let fallback = payload
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(turn_id) = remote_turn_id(payload) else {
        return Ok(fallback);
    };
    let mut events = Vec::new();
    let mut seen = BTreeSet::new();
    let mut last_id = 0_u64;
    for event in fallback {
        last_id = last_id.max(app_event_sequence(&event));
        if let Some(key) = app_event_dedupe_key(&event)
            && !seen.insert(key)
        {
            continue;
        }
        events.push(event);
    }
    let deadline = SystemTime::now() + Duration::from_secs(remote_attach_wait_seconds());
    loop {
        match remote_turn_events(server_url, auth, &turn_id, last_id) {
            Ok(next) => {
                let mut advanced = false;
                for event in next {
                    let seq = app_event_sequence(&event);
                    if seq > last_id {
                        last_id = seq;
                    }
                    if let Some(key) = app_event_dedupe_key(&event)
                        && !seen.insert(key)
                    {
                        continue;
                    }
                    advanced = true;
                    events.push(event);
                }
                if events.iter().any(is_terminal_app_event) {
                    return Ok(events);
                }
                if !advanced && SystemTime::now() >= deadline {
                    return Ok(events);
                }
            }
            Err(error) if events.is_empty() => return Err(error),
            Err(_) => return Ok(events),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn remote_attach_wait_seconds() -> u64 {
    env::var("OPENAGENT_ATTACH_WAIT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30)
}

fn app_event_sequence(event: &Value) -> u64 {
    event
        .get("sequence")
        .or_else(|| event.get("global_sequence"))
        .and_then(Value::as_u64)
        .unwrap_or_default()
}

fn app_event_dedupe_key(event: &Value) -> Option<String> {
    Some(format!(
        "{}:{}:{}",
        app_event_sequence(event),
        event.get("method").and_then(Value::as_str).unwrap_or(""),
        python_json_dumps(event.get("params").unwrap_or(&Value::Null))
    ))
}

fn is_terminal_app_event(event: &Value) -> bool {
    matches!(
        event.get("method").and_then(Value::as_str),
        Some("turn/completed" | "turn/failed" | "turn/interrupted")
    ) || matches!(
        event
            .get("params")
            .and_then(|params| params.get("status"))
            .and_then(Value::as_str),
        Some("completed" | "failed" | "interrupted")
    )
}

fn remote_turn_id(payload: &Value) -> Option<String> {
    payload
        .get("turn_id")
        .or_else(|| payload.get("turn").and_then(|turn| turn.get("id")))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn remote_list_sessions(server_url: &str, auth: &RemoteAuth) -> Result<Vec<Value>, String> {
    let payload = http_json_with_auth("GET", server_url, "/api/sessions", auth, None)?;
    Ok(payload
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn remote_sessions_text(sessions: &[Value]) -> String {
    if sessions.is_empty() {
        return "Remote sessions: none\n".to_string();
    }
    let mut text = String::from("Remote sessions:\n");
    for (index, session) in sessions.iter().take(20).enumerate() {
        let id = session
            .get("session_id")
            .or_else(|| session.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let status = session
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let messages = session
            .get("message_count")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let workspace = session
            .get("workspace")
            .and_then(Value::as_str)
            .unwrap_or(".");
        text.push_str(&format!(
            "  {}. {}  status={}  messages={}  workspace={}\n",
            index + 1,
            id,
            status,
            messages,
            workspace
        ));
    }
    if sessions.len() > 20 {
        text.push_str(&format!("  ... {} more\n", sessions.len() - 20));
    }
    text
}

fn http_json(
    method: &str,
    server_url: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Result<Value, String> {
    http_json_with_auth(
        method,
        server_url,
        path,
        &RemoteAuth {
            token: token.map(str::to_string),
            username: None,
            password: None,
        },
        body,
    )
}

fn http_json_with_auth(
    method: &str,
    server_url: &str,
    path: &str,
    auth: &RemoteAuth,
    body: Option<Value>,
) -> Result<Value, String> {
    let raw = http_text_with_auth(method, server_url, path, auth, body)?;
    serde_json::from_str(&raw).map_err(|error| format!("server response was not JSON: {error}"))
}

fn http_text_with_auth(
    method: &str,
    server_url: &str,
    path: &str,
    auth: &RemoteAuth,
    body: Option<Value>,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let url = format!("{}{}", server_url.trim_end_matches('/'), path);
    let mut request = match method {
        "GET" => client.get(url),
        "POST" => client.post(url),
        _ => return Err(format!("unsupported HTTP method: {method}")),
    };
    if let Some(token) = auth.token.as_deref().filter(|value| !value.is_empty()) {
        request = request.bearer_auth(token);
    }
    if let Some(password) = auth.password.as_deref().filter(|value| !value.is_empty()) {
        request = request.basic_auth(
            auth.username.as_deref().unwrap_or("openagent"),
            Some(password),
        );
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .map_err(|error| format!("{method} {path} failed: {error}"))?;
    let status = response.status();
    let raw = response.text().map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "{method} {path} returned HTTP {}: {raw}",
            status.as_u16()
        ));
    }
    Ok(raw)
}

fn text_from_app_events(events: &[Value]) -> String {
    let mut text = String::new();
    let mut final_answer = String::new();
    for event in events {
        let method = event
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = event.get("params").unwrap_or(&Value::Null);
        if method == "item/agentMessage/delta" {
            if let Some(delta) = params
                .get("delta")
                .or_else(|| params.get("text"))
                .or_else(|| params.get("event").and_then(|event| event.get("text")))
                .and_then(Value::as_str)
            {
                text.push_str(delta);
            }
        }
        if method == "turn/approval_requested" {
            let tool = params
                .get("approval")
                .and_then(|approval| approval.get("tool_name"))
                .and_then(Value::as_str)
                .unwrap_or("tool");
            text.push_str(&format!("\n[approval required: {tool}]\n"));
        }
        if method == "turn/interrupted" {
            text.push_str("\n[turn interrupted]\n");
        }
        if method == "turn/completed" {
            final_answer = params
                .get("final_answer")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
    }
    if text.is_empty() { final_answer } else { text }
}

fn http_runtime_command(args: &[String], web: bool, help: &'static str) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(help);
    }
    let mut runtime_args = args.to_vec();
    if !web && !has_flag(args, &["--headless"]) {
        runtime_args.push("--headless".to_string());
    }
    let result = openagent_http_runtime::run_cli(&runtime_args);
    CliRunResult {
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
    }
}

fn tui_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(tui_help());
    }
    if value_for(args, &["--attach"]).is_some() {
        return attach_command(args);
    }
    if let Some(prompt) = value_for(args, &["--prompt"]) {
        let mut run_args = vec!["--skip-doctor".to_string()];
        run_args.extend(args.iter().filter(|arg| *arg != "--prompt").cloned());
        run_args.push(prompt);
        return run_prompt_command(&run_args);
    }
    if !io::stdin().is_terminal() {
        return ok_text("openagent-tui ready; pass --prompt or use an interactive terminal");
    }
    interactive_local_loop(args)
}

fn attach_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(attach_help());
    }
    let positionals = positional_args(
        args,
        &[
            "--workspace",
            "--dir",
            "--session",
            "-s",
            "--server-token",
            "--server-token-env",
            "--username",
            "-u",
            "--password",
            "-p",
            "--attach",
            "--format",
        ],
    );
    let Some(url) = value_for(args, &["--attach"]).or_else(|| positionals.first().cloned()) else {
        return err_text(2, "openagent attach requires a server URL");
    };
    let auth = remote_auth_from_args(args);
    let mut health = json!({});
    if !has_flag(args, &["--skip-health-check"])
        && let Err(error) =
            http_json_with_auth("GET", &url, "/api/health", &auth, None).map(|payload| {
                health = payload;
            })
    {
        return err_text(1, error);
    }
    if !io::stdin().is_terminal() {
        let sessions = remote_list_sessions(&url, &auth).unwrap_or_default();
        if value_for(args, &["--format"]).as_deref() == Some("json") {
            return CliRunResult::ok_json(&json!({
                "attached": true,
                "server_url": url,
                "health": health,
                "sessions": sessions,
            }));
        }
        let mut output = format!("attached to {url}\n");
        output.push_str(&remote_sessions_text(&sessions));
        return ok_text(output);
    }
    interactive_remote_loop(args, &url, &auth)
}

fn interactive_local_loop(args: &[String]) -> CliRunResult {
    let mut stdout = String::new();
    stdout.push_str("OpenAgent TUI direct mode. Type /exit to quit.\n");
    let mut line = String::new();
    loop {
        line.clear();
        if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let prompt = line.trim();
        if matches!(prompt, "/exit" | "/quit") {
            break;
        }
        if prompt.is_empty() {
            continue;
        }
        let mut run_args = args.to_vec();
        run_args.push("--skip-doctor".to_string());
        run_args.push(prompt.to_string());
        let result = run_prompt_command(&run_args);
        stdout.push_str(&result.stdout);
        if result.exit_code != 0 {
            return CliRunResult {
                exit_code: result.exit_code,
                stdout,
                stderr: result.stderr,
            };
        }
    }
    ok_text(stdout)
}

fn interactive_remote_loop(args: &[String], url: &str, auth: &RemoteAuth) -> CliRunResult {
    let workspace = workspace_from_args(args);
    let mut current_session = match remote_select_session_with_auth(
        url,
        auth,
        value_for(args, &["--session", "-s"]),
        has_flag(args, &["--continue", "-c"]),
        has_flag(args, &["--fork"]),
        &workspace,
    ) {
        Ok(session_id) => Some(session_id),
        Err(error) if has_flag(args, &["--continue", "-c"]) || has_flag(args, &["--fork"]) => {
            return err_text(1, error);
        }
        Err(_) => None,
    };
    if io::stdout().is_terminal() {
        let handler = RemoteTerminalHandler {
            url: url.to_string(),
            auth: auth.clone(),
            workspace,
            current_session,
            last_turn_id: None,
            last_global_event_id: 0,
            pending_events: Vec::new(),
            seen_events: BTreeSet::new(),
        };
        return match openagent_tui::run_terminal_ui(
            openagent_tui::TerminalUiOptions {
                title: format!("OpenAgent remote attach: {url}"),
                status: "connected".to_string(),
            },
            handler,
        ) {
            Ok(()) => ok_text(""),
            Err(error) => err_text(1, error),
        };
    }
    let mut last_turn_id: Option<String> = None;
    let mut stdout = format!(
        "OpenAgent remote attach: {url}\nCommands: /sessions, /resume <id>, /new, /fork, /interrupt [turn_id], /exit\n"
    );
    if let Ok(sessions) = remote_list_sessions(url, auth) {
        stdout.push_str(&remote_sessions_text(&sessions));
    }
    if let Some(session_id) = current_session.as_deref() {
        stdout.push_str(&format!("Current session: {session_id}\n"));
    }
    let mut line = String::new();
    loop {
        line.clear();
        if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let prompt = line.trim();
        if matches!(prompt, "/exit" | "/quit") {
            break;
        }
        if prompt.is_empty() {
            continue;
        }
        if prompt == "/sessions" {
            match remote_list_sessions(url, auth) {
                Ok(sessions) => stdout.push_str(&remote_sessions_text(&sessions)),
                Err(error) => return err_text(1, error),
            }
            continue;
        }
        if let Some(session_id) = prompt.strip_prefix("/resume ").map(str::trim) {
            if session_id.is_empty() {
                stdout.push_str("Usage: /resume <session_id>\n");
            } else {
                current_session = Some(session_id.to_string());
                stdout.push_str(&format!("Current session: {session_id}\n"));
            }
            continue;
        }
        if prompt == "/new" {
            match remote_select_session_with_auth(url, auth, None, false, false, &workspace) {
                Ok(session_id) => {
                    stdout.push_str(&format!("Created session: {session_id}\n"));
                    current_session = Some(session_id);
                }
                Err(error) => return err_text(1, error),
            }
            continue;
        }
        if prompt == "/fork" {
            let Some(base) = current_session.clone() else {
                stdout.push_str("No current session to fork. Use /new or /resume <session_id>.\n");
                continue;
            };
            match remote_select_session_with_auth(url, auth, Some(base), false, true, &workspace) {
                Ok(session_id) => {
                    stdout.push_str(&format!("Forked session: {session_id}\n"));
                    current_session = Some(session_id);
                }
                Err(error) => return err_text(1, error),
            }
            continue;
        }
        if prompt == "/interrupt" || prompt.starts_with("/interrupt ") {
            let turn_id = prompt
                .strip_prefix("/interrupt ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| last_turn_id.clone());
            let Some(turn_id) = turn_id else {
                stdout.push_str("No turn to interrupt. Pass /interrupt <turn_id>.\n");
                continue;
            };
            match http_json_with_auth(
                "POST",
                url,
                &format!("/api/turns/{turn_id}/interrupt"),
                auth,
                None,
            ) {
                Ok(payload) => {
                    if let Some(events) = payload.get("events").and_then(Value::as_array) {
                        stdout.push_str(&text_from_app_events(events));
                        stdout.push('\n');
                    } else {
                        stdout.push_str(&python_json_dumps(&payload));
                        stdout.push('\n');
                    }
                }
                Err(error) => return err_text(1, error),
            }
            continue;
        }
        let session_id = match current_session.clone() {
            Some(session_id) => session_id,
            None => {
                match remote_select_session_with_auth(url, auth, None, false, false, &workspace) {
                    Ok(session_id) => {
                        stdout.push_str(&format!("Created session: {session_id}\n"));
                        current_session = Some(session_id.clone());
                        session_id
                    }
                    Err(error) => return err_text(1, error),
                }
            }
        };
        match remote_start_turn_with_auth(url, auth, &session_id, prompt) {
            Ok(payload) => {
                last_turn_id = remote_turn_id(&payload).or(last_turn_id);
                match remote_events_for_payload(url, auth, &payload) {
                    Ok(events) if !events.is_empty() => {
                        stdout.push_str(&text_from_app_events(&events));
                        stdout.push('\n');
                    }
                    Ok(_) => {
                        stdout.push_str(&python_json_dumps(&payload));
                        stdout.push('\n');
                    }
                    Err(error) => return err_text(1, error),
                }
            }
            Err(error) => return err_text(1, error),
        }
    }
    ok_text(stdout)
}

struct RemoteTerminalHandler {
    url: String,
    auth: RemoteAuth,
    workspace: PathBuf,
    current_session: Option<String>,
    last_turn_id: Option<String>,
    last_global_event_id: u64,
    pending_events: Vec<Value>,
    seen_events: BTreeSet<String>,
}

impl RemoteTerminalHandler {
    fn ensure_session(&mut self) -> Result<String, String> {
        if let Some(session_id) = self.current_session.clone() {
            return Ok(session_id);
        }
        let session_id = remote_select_session_with_auth(
            &self.url,
            &self.auth,
            None,
            false,
            false,
            &self.workspace,
        )?;
        self.current_session = Some(session_id.clone());
        Ok(session_id)
    }

    fn remember_payload_events(&mut self, payload: &Value) {
        let events = payload
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let events = self.filter_new_events(events);
        self.pending_events.extend(events);
    }

    fn filter_new_events(&mut self, events: Vec<Value>) -> Vec<Value> {
        let mut output = Vec::new();
        for event in events {
            let sequence = app_event_sequence(&event);
            if sequence > self.last_global_event_id {
                self.last_global_event_id = sequence;
            }
            if let Some(key) = app_event_dedupe_key(&event)
                && !self.seen_events.insert(key)
            {
                continue;
            }
            output.push(event);
        }
        output
    }
}

impl openagent_tui::TerminalEventHandler for RemoteTerminalHandler {
    fn initial_lines(&mut self) -> Vec<openagent_tui::TimelineLine> {
        let mut lines = tui_lines("status", format!("connected to {}", self.url), true);
        match remote_list_sessions(&self.url, &self.auth) {
            Ok(sessions) => {
                lines.extend(tui_lines("status", remote_sessions_text(&sessions), false))
            }
            Err(error) => lines.extend(tui_lines("warning", error, true)),
        }
        if let Some(session_id) = self.current_session.as_deref() {
            lines.extend(tui_lines(
                "status",
                format!("current session: {session_id}"),
                true,
            ));
        }
        lines
    }

    fn poll_app_events(&mut self) -> Result<Vec<Value>, String> {
        let raw = http_text_with_auth(
            "GET",
            &self.url,
            &format!("/api/events?last_event_id={}", self.last_global_event_id),
            &self.auth,
            None,
        )?;
        let events =
            openagent_http_runtime::parse_sse_response_lines(&raw.lines().collect::<Vec<_>>())?;
        Ok(self.filter_new_events(events))
    }

    fn poll_control_request(&mut self) -> Result<Option<Value>, String> {
        let payload = http_json_with_auth("GET", &self.url, "/tui/control/next", &self.auth, None)?;
        let path = payload
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if path.is_empty() {
            return Ok(None);
        }
        Ok(Some(payload))
    }

    fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
        http_json_with_auth(
            "POST",
            &self.url,
            "/tui/control/response",
            &self.auth,
            Some(payload.clone()),
        )
        .map(|_| ())
    }

    fn drain_app_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.pending_events)
    }

    fn handle_submit(&mut self, prompt: &str) -> Result<Vec<openagent_tui::TimelineLine>, String> {
        let session_id = self.ensure_session()?;
        let payload = remote_start_turn_with_auth(&self.url, &self.auth, &session_id, prompt)?;
        self.last_turn_id = remote_turn_id(&payload).or_else(|| self.last_turn_id.clone());
        let events = remote_events_for_payload(&self.url, &self.auth, &payload)?;
        if events.is_empty() {
            return Ok(tui_lines("assistant", python_json_dumps(&payload), false));
        }
        let events = self.filter_new_events(events);
        self.pending_events.extend(events);
        Ok(Vec::new())
    }

    fn handle_command(
        &mut self,
        command: &str,
    ) -> Result<Vec<openagent_tui::TimelineLine>, String> {
        if command == "/sessions" {
            let sessions = remote_list_sessions(&self.url, &self.auth)?;
            return Ok(tui_lines("status", remote_sessions_text(&sessions), false));
        }
        if let Some(session_id) = command.strip_prefix("/resume ").map(str::trim) {
            if session_id.is_empty() {
                return Ok(tui_lines("warning", "usage: /resume <session_id>", true));
            }
            self.current_session = Some(session_id.to_string());
            return Ok(tui_lines(
                "status",
                format!("current session: {session_id}"),
                true,
            ));
        }
        if command == "/new" {
            let session_id = remote_select_session_with_auth(
                &self.url,
                &self.auth,
                None,
                false,
                false,
                &self.workspace,
            )?;
            self.current_session = Some(session_id.clone());
            return Ok(tui_lines(
                "status",
                format!("created session: {session_id}"),
                true,
            ));
        }
        if command == "/fork" {
            let Some(base) = self.current_session.clone() else {
                return Ok(tui_lines(
                    "warning",
                    "no current session to fork; use /new or /resume <session_id>",
                    true,
                ));
            };
            let session_id = remote_select_session_with_auth(
                &self.url,
                &self.auth,
                Some(base),
                false,
                true,
                &self.workspace,
            )?;
            self.current_session = Some(session_id.clone());
            return Ok(tui_lines(
                "status",
                format!("forked session: {session_id}"),
                true,
            ));
        }
        if command == "/interrupt" || command.starts_with("/interrupt ") {
            let turn_id = command
                .strip_prefix("/interrupt ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| self.last_turn_id.clone());
            let Some(turn_id) = turn_id else {
                return Ok(tui_lines("warning", "no turn to interrupt", true));
            };
            let payload = http_json_with_auth(
                "POST",
                &self.url,
                &format!("/api/turns/{turn_id}/interrupt"),
                &self.auth,
                None,
            )?;
            self.remember_payload_events(&payload);
            return Ok(Vec::new());
        }
        Ok(tui_lines(
            "status",
            "commands: /sessions, /resume <id>, /new, /fork, /interrupt [turn_id], /exit",
            false,
        ))
    }

    fn handle_approval_response(
        &mut self,
        payload: &Value,
    ) -> Result<Vec<openagent_tui::TimelineLine>, String> {
        let turn_id = payload
            .get("turn_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "approval response missing turn_id".to_string())?;
        let request_id = payload
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "approval response missing request_id".to_string())?;
        let response = http_json_with_auth(
            "POST",
            &self.url,
            &format!("/api/turns/{turn_id}/approvals/{request_id}"),
            &self.auth,
            Some(payload.clone()),
        )?;
        self.remember_payload_events(&response);
        Ok(Vec::new())
    }

    fn handle_question_response(
        &mut self,
        payload: &Value,
    ) -> Result<Vec<openagent_tui::TimelineLine>, String> {
        let turn_id = payload
            .get("turn_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "question response missing turn_id".to_string())?;
        let request_id = payload
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "question response missing request_id".to_string())?;
        let response = http_json_with_auth(
            "POST",
            &self.url,
            &format!("/api/turns/{turn_id}/questions/{request_id}/reply"),
            &self.auth,
            Some(payload.clone()),
        )?;
        self.remember_payload_events(&response);
        Ok(Vec::new())
    }
}

fn tui_lines(
    kind: &str,
    text: impl Into<String>,
    important: bool,
) -> Vec<openagent_tui::TimelineLine> {
    let text = text.into();
    if text.trim().is_empty() {
        return Vec::new();
    }
    text.lines()
        .map(|line| openagent_tui::TimelineLine::new(kind, line.to_string(), important))
        .collect()
}

fn agent_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(
            "Usage: openagent agent <list|create|show|delete|run> [name] [--model <id>] [--provider <id>] [--mode <primary|subagent>] [--permission <ruleset>] [--prompt <text>] [--tool <name>]",
        );
    }
    match args[0].as_str() {
        "list" | "ls" => {
            let dir = agent_registry_dir(args);
            let agents = fs::read_dir(&dir)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    (path.extension().and_then(|value| value.to_str()) == Some("json"))
                        .then(|| read_json_file(&path))
                })
                .collect::<Vec<_>>();
            CliRunResult::ok_json(&json!({"agents": agents}))
        }
        "create" => {
            let positionals = positional_args(
                args,
                &[
                    "--workspace",
                    "--dir",
                    "--model",
                    "-m",
                    "--provider",
                    "--mode",
                    "--permission",
                    "--description",
                    "--prompt",
                    "--tool",
                    "--format",
                ],
            );
            let Some(name) = positionals.get(1).or_else(|| positionals.first()) else {
                return err_text(2, "agent create requires a name");
            };
            let agent_id = sanitize_identifier(name);
            let mode = value_for(args, &["--mode"]).unwrap_or_else(|| "primary".to_string());
            if !matches!(mode.as_str(), "primary" | "subagent") {
                return err_text(2, "agent mode must be primary or subagent");
            }
            let dir = agent_registry_dir(args);
            let path = dir.join(format!("{agent_id}.json"));
            let payload = json!({
                "schema_version": "openagent.agent.v1",
                "id": agent_id,
                "name": name,
                "model": value_for(args, &["--model", "-m"]),
                "provider": value_for(args, &["--provider"]),
                "mode": mode,
                "permission": value_for(args, &["--permission"]).unwrap_or_else(|| "ask".to_string()),
                "description": value_for(args, &["--description"]),
                "prompt": value_for(args, &["--prompt"]),
                "tools": values_for(args, &["--tool"]),
                "updated_at_ms": now_ms_cli(),
            });
            if let Err(error) = write_json_file(&path, &payload) {
                return err_text(1, error);
            }
            CliRunResult::ok_json(
                &json!({"created": true, "path": path.to_string_lossy(), "agent": payload}),
            )
        }
        "show" => {
            let positionals = positional_args(args, &["--workspace", "--dir", "--format"]);
            let Some(name) = positionals.get(1).or_else(|| positionals.first()) else {
                return err_text(2, "agent show requires a name");
            };
            let path = agent_registry_dir(args).join(format!("{}.json", sanitize_identifier(name)));
            CliRunResult::ok_json(&read_json_file(&path))
        }
        "delete" | "rm" => {
            let positionals = positional_args(args, &["--workspace", "--dir", "--format"]);
            let Some(name) = positionals.get(1).or_else(|| positionals.first()) else {
                return err_text(2, "agent delete requires a name");
            };
            let path = agent_registry_dir(args).join(format!("{}.json", sanitize_identifier(name)));
            let removed = fs::remove_file(&path).is_ok();
            CliRunResult::ok_json(&json!({"removed": removed, "path": path.to_string_lossy()}))
        }
        "run" => {
            let positionals = positional_args(
                args,
                &[
                    "--workspace",
                    "--dir",
                    "--session-root",
                    "--format",
                    "--model",
                    "-m",
                    "--provider",
                    "--api-key",
                    "--base-url",
                    "--wire-api",
                    "--mcp-config",
                ],
            );
            let Some(name) = positionals.get(1).or_else(|| positionals.first()) else {
                return err_text(2, "agent run requires an agent name");
            };
            let prompt = positionals
                .iter()
                .skip(if positionals.get(1).is_some() { 2 } else { 1 })
                .cloned()
                .collect::<Vec<_>>();
            if prompt.is_empty() {
                return err_text(2, "agent run requires a prompt");
            }
            let mut run_args = Vec::new();
            copy_cli_options(
                args,
                &[
                    "--workspace",
                    "--dir",
                    "--session-root",
                    "--format",
                    "--model",
                    "-m",
                    "--provider",
                    "--api-key",
                    "--base-url",
                    "--wire-api",
                    "--skip-doctor",
                    "--stream",
                    "--mcp-config",
                ],
                &mut run_args,
            );
            run_args.push("--agent".to_string());
            run_args.push(sanitize_identifier(name));
            run_args.extend(prompt);
            run_prompt_command(&run_args)
        }
        other => err_text(2, format!("unknown agent command: {other}")),
    }
}

fn plugin_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(
            "Usage: openagent plugin <install|list|show|enable|disable|remove|run> [module-or-path] [--global] [--force]",
        );
    }
    match args[0].as_str() {
        "install" | "add" => plugin_install(&args[1..]),
        "list" | "ls" => plugin_list(&args[1..]),
        "show" => plugin_show(&args[1..]),
        "enable" => plugin_set_enabled(&args[1..], true),
        "disable" => plugin_set_enabled(&args[1..], false),
        "remove" | "rm" | "uninstall" => plugin_remove(&args[1..]),
        "run" => plugin_run(&args[1..]),
        _ => plugin_install(args),
    }
}

fn plugin_install(args: &[String]) -> CliRunResult {
    let module = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .cloned()
        .unwrap_or_default();
    if module.is_empty() {
        return err_text(2, "plugin install requires a module or path");
    }
    let path = if has_flag(args, &["--global", "-g"]) {
        home_dir().join(".config/openagent/plugins.json")
    } else {
        workspace_from_args(args).join(".openagent/plugins.json")
    };
    let mut config = read_json_file(&path);
    let plugins = ensure_object_field(&mut config, "plugins");
    let manifest = plugin_manifest_from_source(&module);
    let plugin_id = manifest
        .get("id")
        .or_else(|| manifest.get("name"))
        .and_then(Value::as_str)
        .map(sanitize_identifier)
        .unwrap_or_else(|| sanitize_identifier(&module));
    if plugins.contains_key(&plugin_id) && !has_flag(args, &["--force", "-f"]) {
        return err_text(1, format!("plugin already registered: {plugin_id}"));
    }
    plugins.insert(
        plugin_id.clone(),
        json!({
            "schema_version": "openagent.plugin_install.v1",
            "id": plugin_id,
            "module": module,
            "source": plugin_source_kind(&module),
            "enabled": true,
            "manifest": manifest,
            "updated_at_ms": now_ms_cli(),
        }),
    );
    if let Err(error) = write_json_file(&path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"installed": true, "path": path.to_string_lossy(), "plugin_id": plugin_id}),
    )
}

fn plugin_list(args: &[String]) -> CliRunResult {
    let path = plugin_registry_path(args);
    let config = read_json_file(&path);
    let plugins = config
        .get("plugins")
        .and_then(Value::as_object)
        .map(|items| items.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    CliRunResult::ok_json(&json!({"path": path.to_string_lossy(), "plugins": plugins}))
}

fn plugin_show(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--format"]);
    let Some(id) = positionals.first() else {
        return err_text(2, "plugin show requires an id");
    };
    let path = plugin_registry_path(args);
    let plugin_id = sanitize_identifier(id);
    let plugin = read_json_file(&path)
        .get("plugins")
        .and_then(|plugins| plugins.get(&plugin_id))
        .cloned()
        .unwrap_or(Value::Null);
    CliRunResult::ok_json(&json!({"path": path.to_string_lossy(), "plugin": plugin}))
}

fn plugin_set_enabled(args: &[String], enabled: bool) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--format"]);
    let Some(id) = positionals.first() else {
        return err_text(2, "plugin enable/disable requires an id");
    };
    let path = plugin_registry_path(args);
    let mut config = read_json_file(&path);
    let plugins = ensure_object_field(&mut config, "plugins");
    let plugin_id = sanitize_identifier(id);
    let Some(plugin) = plugins.get_mut(&plugin_id).and_then(Value::as_object_mut) else {
        return err_text(1, format!("plugin not installed: {plugin_id}"));
    };
    plugin.insert("enabled".to_string(), json!(enabled));
    plugin.insert("updated_at_ms".to_string(), json!(now_ms_cli()));
    if let Err(error) = write_json_file(&path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(&json!({"plugin_id": plugin_id, "enabled": enabled}))
}

fn plugin_remove(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--format"]);
    let Some(id) = positionals.first() else {
        return err_text(2, "plugin remove requires an id");
    };
    let path = plugin_registry_path(args);
    let mut config = read_json_file(&path);
    let removed = config
        .get_mut("plugins")
        .and_then(Value::as_object_mut)
        .and_then(|plugins| plugins.remove(&sanitize_identifier(id)))
        .is_some();
    if let Err(error) = write_json_file(&path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(&json!({"removed": removed, "path": path.to_string_lossy()}))
}

fn plugin_run(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--format"]);
    let Some(id) = positionals.first() else {
        return err_text(2, "plugin run requires an id");
    };
    let command = positionals
        .get(1)
        .cloned()
        .unwrap_or_else(|| "default".to_string());
    let path = plugin_registry_path(args);
    let plugin_id = sanitize_identifier(id);
    let plugin = read_json_file(&path)
        .get("plugins")
        .and_then(|plugins| plugins.get(&plugin_id))
        .cloned()
        .unwrap_or(Value::Null);
    if plugin.is_null() {
        return err_text(1, format!("plugin not installed: {plugin_id}"));
    }
    CliRunResult::ok_json(&json!({
        "plugin_id": plugin_id,
        "command": command,
        "executed": false,
        "reason": "plugin command execution is planned explicitly; registry and manifest resolution are complete",
        "plugin": plugin,
    }))
}

fn agent_registry_dir(args: &[String]) -> PathBuf {
    workspace_from_args(args).join(".openagent/agents")
}

fn plugin_registry_path(args: &[String]) -> PathBuf {
    if has_flag(args, &["--global", "-g"]) {
        home_dir().join(".config/openagent/plugins.json")
    } else {
        workspace_from_args(args).join(".openagent/plugins.json")
    }
}

fn plugin_manifest_from_source(source: &str) -> Value {
    let path = PathBuf::from(source);
    if path.is_dir() {
        for relative in [".codex-plugin/plugin.json", "plugin.json"] {
            let value = read_json_file(&path.join(relative));
            if value.as_object().is_some_and(|object| !object.is_empty()) {
                return value;
            }
        }
    } else if path.is_file() {
        let value = read_json_file(&path);
        if value.as_object().is_some_and(|object| !object.is_empty()) {
            return value;
        }
    }
    json!({
        "id": sanitize_identifier(source),
        "name": source,
        "source": source,
        "capabilities": [],
        "commands": {},
    })
}

fn plugin_source_kind(source: &str) -> &'static str {
    let path = PathBuf::from(source);
    if path.exists() {
        "local"
    } else if source.starts_with("http://")
        || source.starts_with("https://")
        || source.contains('/')
    {
        "remote"
    } else {
        "module"
    }
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
    if object
        .get("native")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return format!(
            "OpenAgent doctor\n- provider: {} ({})\n- model: {}\n- api key: {} ({})\n- base_url: {}\n- dependency: {} ({})\n- model endpoint: skipped ({})\n",
            string_field(object, "provider"),
            string_field(object, "provider_label"),
            string_field(object, "model"),
            if bool_field(object, "api_key_set") {
                "set"
            } else {
                "missing"
            },
            string_field(object, "api_key_env"),
            string_field(object, "base_url"),
            if bool_field(object, "dependency_ok") {
                "ok"
            } else {
                "missing"
            },
            string_field(object, "dependency_message"),
            string_field(object, "model_endpoint_message"),
        );
    }
    format!(
        "OpenAgent doctor\n- provider: {} ({})\n- OPENAI_BASE_URL: {}\n- OPENAI_MODEL: {}\n- OPENAI_WIRE_API: {}\n- {}: {}\n- model endpoint: {} ({})\n",
        string_field(object, "provider"),
        string_field(object, "provider_label"),
        string_field(object, "base_url"),
        string_field(object, "model"),
        string_field(object, "wire_api"),
        string_field(object, "api_key_env"),
        if bool_field(object, "api_key_set") {
            "set"
        } else {
            "missing"
        },
        if bool_field(object, "model_endpoint_ok") {
            "ok"
        } else {
            "failed"
        },
        string_field(object, "model_endpoint_message"),
    )
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
