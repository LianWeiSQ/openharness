//! CLI crate for the Rust rewrite.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{self, IsTerminal, Read},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use openagent_protocol::{ChatMessage, Role, Usage};
use openagent_provider::{
    AnthropicLanguageModelConfig, OpenAiLanguageModelConfig, ProviderStreamEvent, anthropic_model,
    build_anthropic_payload, build_openai_chat_payload, build_openai_responses_payload,
    default_env_mapping, normalize_openai_responses_response, normalize_provider,
    openai_compatible_model, provider_auth_methods, provider_default_base_url,
    provider_default_model, provider_label, provider_requires_api_key, summarize_http_error_body,
};
use openagent_session::{
    FileSessionStore, Session, SessionEventOptions, SessionPartOptions, SessionStatus,
    StartRunOptions,
};
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
       --dir, --workspace <path>        workspace path\n\
       --session-root <path>            session store root\n\
       --variant <name>                 provider-specific variant\n\
       --thinking                       show thinking blocks\n\
       --interactive, -i                run direct interactive mode\n\
       --dangerously-skip-permissions   auto-approve permissions that are not denied\n\
       --skip-doctor                    skip local gateway check"
}

fn tui_help() -> &'static str {
    "Usage: openagent tui [options]\n\n\
     Options: --workspace <path>, --session-root <path>, -s/--session <id>, -c/--continue, --fork, --model <provider/model>, --agent <name>, --prompt <text>, --skip-doctor"
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
     Options: --workspace <path>, -s/--session <id>, -c/--continue, --fork, --skip-health-check, --server-token <token>, -u/--username <name>, -p/--password <password>"
}

fn doctor_help() -> &'static str {
    "Usage: openagent doctor [options]\n\n\
     Options: --format <text|json>, --base-url <url>, --model <id>, --wire-api <chat|responses>, --api-key <key>"
}

fn models_help() -> &'static str {
    "Usage: openagent models [provider] [options]\n\n\
     Options: --format <table|json>, --refresh, --verbose"
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

fn run_prompt_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(run_help());
    }
    let format = value_for(args, &["--format"]).unwrap_or_else(|| "text".to_string());
    let (provider, model_id) = provider_and_model_from_args(args);
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
    let message = positional_args(
        args,
        &[
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
            "--max-steps",
            "--agent",
            "--title",
            "--attach",
            "--password",
            "-p",
            "--username",
            "-u",
            "--variant",
            "--port",
        ],
    )
    .join(" ");
    let stdin_text = read_piped_stdin();
    let message = if message.trim().is_empty() {
        stdin_text.trim().to_string()
    } else if stdin_text.trim().is_empty() {
        message
    } else {
        format!("{}\n{}", message.trim(), stdin_text.trim())
    };
    if message.trim().is_empty() {
        return err_text(2, "openagent run requires a prompt or piped stdin");
    }
    let workspace = workspace_from_args(args);
    let files = match attached_files(&workspace, &values_for(args, &["--file", "-f"])) {
        Ok(files) => files,
        Err(error) => return err_text(1, error),
    };
    let prompt = build_prompt_with_files(&message, &files);
    let agent_name = value_for(args, &["--agent"]).unwrap_or_else(|| "default".to_string());
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
        json!(has_flag(args, &["--dangerously-skip-permissions"])),
    );
    if let Err(error) = store.start_run(
        &mut session,
        StartRunOptions {
            run_id: run_id.clone(),
            trace_id,
            agent_name,
            model_id: Some(model_id.clone()),
            provider_id: Some(provider.clone()),
            permission: if has_flag(args, &["--dangerously-skip-permissions"]) {
                "auto_allow".to_string()
            } else {
                "ask".to_string()
            },
            max_steps,
            started_at_ms: None,
        },
    ) {
        return err_text(1, format!("failed to start session run: {error}"));
    }
    let user_message = chat_message(Role::User, prompt.clone());
    let user_index = session.messages.len() as u64;
    session.add(user_message.clone());
    if let Err(error) = store.append_message(&session, &user_message, &run_id, user_index) {
        return err_text(1, format!("failed to record user message: {error}"));
    }
    let provider_result = call_provider_for_run(args, &provider, &model_id, &session.messages);
    let provider_result = match provider_result {
        Ok(result) => result,
        Err(error) => {
            session.status = SessionStatus::Stop;
            let _ = store.finish_run(&session, &run_id, "failed", 1, Some("error"), Some(&error));
            return err_text(1, error);
        }
    };
    let answer = provider_result.answer;
    let assistant_message = chat_message(Role::Assistant, answer.clone());
    let assistant_index = session.messages.len() as u64;
    session.add(assistant_message.clone());
    session.status = SessionStatus::Idle;
    if let Err(error) = store.append_message(&session, &assistant_message, &run_id, assistant_index)
    {
        return err_text(1, format!("failed to record assistant message: {error}"));
    }
    let _ = store.record_event(
        &session.id,
        &run_id,
        "model.usage",
        SessionEventOptions {
            kind: "model".to_string(),
            attributes: BTreeMap::from([
                (
                    "input_tokens".to_string(),
                    json!(provider_result.usage.input_tokens),
                ),
                (
                    "output_tokens".to_string(),
                    json!(provider_result.usage.output_tokens),
                ),
                ("cost".to_string(), json!(provider_result.usage.cost)),
                ("source".to_string(), json!(provider_result.source.clone())),
            ]),
            ..SessionEventOptions::default()
        },
    );
    let _ = store.append_part(
        &session.id,
        &run_id,
        "text",
        SessionPartOptions {
            attributes: BTreeMap::from([
                ("role".to_string(), json!("assistant")),
                ("chars".to_string(), json!(answer.chars().count())),
            ]),
            step_index: Some(1),
            ..SessionPartOptions::default()
        },
    );
    if let Err(error) = store.finish_run(&session, &run_id, "completed", 1, Some("stop"), None) {
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
                "source": provider_result.source,
                "forked": forked,
            }
        });
        if let Some(share) = share {
            completed["params"]["share"] = share;
        }
        let events = [
            json!({"method": "item/agentMessage/delta", "params": {"delta": answer.clone(), "prompt": prompt, "session_id": session.id.clone(), "run_id": run_id.clone()}}),
            completed,
        ];
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
    if has_flag(args, &["--refresh"])
        && let Err(error) = refresh_models_cache()
    {
        return err_text(1, error);
    }
    let provider = positional_args(args, &["--format"])
        .first()
        .cloned()
        .unwrap_or_else(active_provider);
    let normalized = match normalize_provider(Some(&provider)) {
        Ok(provider) => provider,
        Err(error) => return err_text(2, error),
    };
    let verbose = has_flag(args, &["--verbose"]);
    let cached = load_cached_provider_models(&normalized);
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
    let payload = json!({
        "provider": normalized,
        "provider_label": provider_label(&provider).unwrap_or(provider),
        "models": models,
        "cache_path": models_cache_path().to_string_lossy(),
        "refreshed": has_flag(args, &["--refresh"]),
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
                    format!("{id}\n{}", python_json_dumps(model))
                } else {
                    id.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        ok_text(format!(
            "provider: {}\n{}",
            payload["provider"].as_str().unwrap_or("openai"),
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
    let config = read_json_file(&config_path);
    let refresh = has_flag(args, &["--refresh"]);
    let servers = mcp_public_servers(&config)
        .into_iter()
        .map(|mut server| {
            if let Some(object) = server.as_object_mut() {
                let probe = if refresh {
                    probe_url(
                        object
                            .get("url")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
                } else {
                    Ok("not refreshed".to_string())
                };
                let ok = probe.is_ok();
                object.insert(
                    "status".to_string(),
                    json!(if refresh {
                        if ok { "reachable" } else { "failed" }
                    } else {
                        "idle"
                    }),
                );
                object.insert("tool_count".to_string(), json!(0));
                object.insert(
                    "last_error".to_string(),
                    probe
                        .as_ref()
                        .err()
                        .map_or(Value::Null, |error| json!(error)),
                );
                object.insert(
                    "last_refreshed_at".to_string(),
                    if refresh {
                        json!(now_ms_cli())
                    } else {
                        Value::Null
                    },
                );
                object.insert("tools".to_string(), json!([]));
                object.insert("ok".to_string(), json!(ok));
            }
            server
        })
        .collect::<Vec<_>>();
    let payload = json!({
        "config_path": config_path.to_string_lossy(),
        "configured": !servers.is_empty(),
        "enabled": servers.iter().any(|server| server["enabled"].as_bool().unwrap_or(false)),
        "server_count": servers.len(),
        "ok": servers.iter().all(|server| server["ok"].as_bool().unwrap_or(false)),
        "refresh_error": servers.iter().find_map(|server| server["last_error"].as_str()).map(str::to_string),
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
    usage: Usage,
    source: String,
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

fn provider_and_model_from_args(args: &[String]) -> (String, String) {
    if let Some(raw) = value_for(args, &["--model", "-m"])
        && let Some((provider, model)) = raw.split_once('/')
        && !provider.is_empty()
        && !model.is_empty()
    {
        let provider = normalize_provider(Some(provider)).unwrap_or_else(|_| provider.to_string());
        return (provider, model.to_string());
    }
    let provider = value_for(args, &["--provider"]).unwrap_or_else(active_provider);
    let provider = normalize_provider(Some(&provider)).unwrap_or(provider);
    let model = value_for(args, &["--model", "-m"])
        .or_else(|| provider_env_value(&provider, "model"))
        .unwrap_or_else(|| default_model_for_provider(&provider));
    (provider, model)
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

fn call_provider_for_run(
    args: &[String],
    provider: &str,
    model_id: &str,
    messages: &[ChatMessage],
) -> Result<ProviderRunResult, String> {
    if let Ok(answer) = env::var("OPENAGENT_MOCK_ANSWER")
        && !answer.is_empty()
    {
        return Ok(ProviderRunResult {
            answer,
            usage: Usage::default(),
            source: "mock".to_string(),
        });
    }
    let api_key = provider_api_key(provider, args);
    if provider_requires_api_key(provider).unwrap_or(true) && api_key.is_none() {
        return Ok(ProviderRunResult {
            answer: "hello from openagent".to_string(),
            usage: Usage::default(),
            source: "offline_fallback_missing_api_key".to_string(),
        });
    }
    let api_key = api_key.unwrap_or_default();
    if provider == "anthropic" {
        call_anthropic_provider(args, &api_key, model_id, messages)
    } else {
        call_openai_compatible_provider(args, provider, &api_key, model_id, messages)
    }
}

fn call_openai_compatible_provider(
    args: &[String],
    provider: &str,
    api_key: &str,
    model_id: &str,
    messages: &[ChatMessage],
) -> Result<ProviderRunResult, String> {
    let base_url = provider_base_url(provider, args);
    if is_synthetic_endpoint(&base_url) {
        return Ok(ProviderRunResult {
            answer: "hello from openagent".to_string(),
            usage: Usage::default(),
            source: "offline_fallback_synthetic_endpoint".to_string(),
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
    let (endpoint, mut payload) = if wire_api == "chat" {
        let mut payload = build_openai_chat_payload(&config, None, messages, &[], None, None, None);
        if let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(false));
        }
        (join_url(&base_url, "chat/completions"), payload)
    } else {
        let payload = build_openai_responses_payload(&config, None, messages, &[], None, None);
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
    let response = client
        .post(endpoint)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
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
    let (answer, usage) = if wire_api == "chat" {
        (
            extract_chat_answer(&value),
            usage_from_json(value.get("usage")),
        )
    } else {
        let events = normalize_openai_responses_response(&value);
        provider_events_to_answer_usage(&events)
    };
    Ok(ProviderRunResult {
        answer: if answer.is_empty() {
            python_json_dumps(&value)
        } else {
            answer
        },
        usage,
        source: format!("{provider}:{wire_api}"),
    })
}

fn call_anthropic_provider(
    args: &[String],
    api_key: &str,
    model_id: &str,
    messages: &[ChatMessage],
) -> Result<ProviderRunResult, String> {
    let timeout = Duration::from_secs(60);
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())?;
    let mut config = AnthropicLanguageModelConfig::new(api_key, model_id);
    config.base_url =
        value_for(args, &["--base-url"]).or_else(|| provider_env_value("anthropic", "base_url"));
    let mut payload = build_anthropic_payload(&config, None, messages, &[], None, None, None);
    if let Some(object) = payload.as_object_mut() {
        object.insert("stream".to_string(), json!(false));
    }
    let endpoint = join_url(
        config
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com/v1"),
        "messages",
    );
    let response = client
        .post(endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
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
    Ok(ProviderRunResult {
        answer,
        usage: usage_from_json(value.get("usage")),
        source: "anthropic:messages".to_string(),
    })
}

fn provider_events_to_answer_usage(events: &[ProviderStreamEvent]) -> (String, Usage) {
    let mut answer = String::new();
    let mut usage = Usage::default();
    for event in events {
        match event {
            ProviderStreamEvent::TextDelta { text } => answer.push_str(text),
            ProviderStreamEvent::Finish { usage: item, .. } => usage = item.clone(),
            ProviderStreamEvent::ToolCall { .. } => {}
        }
    }
    (answer, usage)
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

fn refresh_models_cache() -> Result<(), String> {
    let url = env::var("OPENAGENT_MODELS_URL").unwrap_or_else(|_| "https://models.dev".to_string());
    let endpoint = join_url(&url, "api.json");
    let raw = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?
        .get(endpoint)
        .send()
        .map_err(|error| format!("failed to fetch models cache: {error}"))?
        .text()
        .map_err(|error| format!("failed to read models cache: {error}"))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("models cache response was not JSON: {error}"))?;
    write_json_file(&models_cache_path(), &value)
}

fn load_cached_provider_models(provider: &str) -> Option<Vec<Value>> {
    let cache = read_json_file(&models_cache_path());
    let provider = cache.get(provider)?;
    let models = provider.get("models")?.as_object()?;
    Some(
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
    )
}

fn models_cache_path() -> PathBuf {
    env::var("OPENAGENT_MODELS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".cache/openagent/models.json"))
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

fn http_json(
    method: &str,
    server_url: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Result<Value, String> {
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
    if let Some(token) = token.filter(|value| !value.is_empty()) {
        request = request.bearer_auth(token);
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
    serde_json::from_str(&raw).map_err(|error| format!("server response was not JSON: {error}"))
}

fn probe_url(url: &str) -> Result<String, String> {
    if url.is_empty() {
        return Err("missing URL".to_string());
    }
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())?
        .get(url)
        .send()
        .map_err(|error| error.to_string())?;
    Ok(format!("HTTP {}", response.status().as_u16()))
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
                .and_then(Value::as_str)
            {
                text.push_str(delta);
            }
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
            "--username",
            "-u",
            "--password",
            "-p",
        ],
    );
    let Some(url) = positionals.first() else {
        return err_text(2, "openagent attach requires a server URL");
    };
    let token =
        value_for(args, &["--server-token"]).or_else(|| env::var(DEFAULT_SERVER_TOKEN_ENV).ok());
    if !has_flag(args, &["--skip-health-check"])
        && let Err(error) = http_json("GET", url, "/api/health", token.as_deref(), None)
    {
        return err_text(1, error);
    }
    if !io::stdin().is_terminal() {
        return ok_text(format!("attached to {url}"));
    }
    interactive_remote_loop(args, url, token.as_deref())
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

fn interactive_remote_loop(args: &[String], url: &str, token: Option<&str>) -> CliRunResult {
    let mut stdout = format!("OpenAgent remote attach: {url}. Type /exit to quit.\n");
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
        let session_id = match remote_select_session(
            url,
            token,
            value_for(args, &["--session", "-s"]),
            has_flag(args, &["--continue", "-c"]),
            &workspace_from_args(args),
        ) {
            Ok(value) => value,
            Err(error) => return err_text(1, error),
        };
        match remote_start_turn(url, token, &session_id, prompt) {
            Ok(payload) => {
                if let Some(events) = payload.get("events").and_then(Value::as_array) {
                    stdout.push_str(&text_from_app_events(events));
                    stdout.push('\n');
                }
            }
            Err(error) => return err_text(1, error),
        }
    }
    ok_text(stdout)
}

fn agent_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(
            "Usage: openagent agent <list|create|show> [name] [--model <id>] [--mode <mode>] [--permission <ruleset>]",
        );
    }
    match args[0].as_str() {
        "list" | "ls" => {
            let dir = workspace_from_args(args).join(".openagent/agents");
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
                    "--mode",
                    "--permission",
                    "--format",
                ],
            );
            let Some(name) = positionals.get(1).or_else(|| positionals.first()) else {
                return err_text(2, "agent create requires a name");
            };
            let dir = workspace_from_args(args).join(".openagent/agents");
            let path = dir.join(format!("{name}.json"));
            let payload = json!({
                "name": name,
                "model": value_for(args, &["--model", "-m"]),
                "mode": value_for(args, &["--mode"]).unwrap_or_else(|| "primary".to_string()),
                "permission": value_for(args, &["--permission"]).unwrap_or_else(|| "ask".to_string()),
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
            let path = workspace_from_args(args).join(format!(".openagent/agents/{name}.json"));
            CliRunResult::ok_json(&read_json_file(&path))
        }
        other => err_text(2, format!("unknown agent command: {other}")),
    }
}

fn plugin_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent plugin <module> [--global] [--force]");
    }
    let module = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .cloned()
        .unwrap_or_default();
    if module.is_empty() {
        return err_text(2, "plugin module is required");
    }
    let path = if has_flag(args, &["--global", "-g"]) {
        home_dir().join(".config/openagent/plugins.json")
    } else {
        workspace_from_args(args).join(".openagent/plugins.json")
    };
    let mut config = read_json_file(&path);
    let plugins = ensure_object_field(&mut config, "plugins");
    if plugins.contains_key(&module) && !has_flag(args, &["--force", "-f"]) {
        return err_text(1, format!("plugin already registered: {module}"));
    }
    plugins.insert(
        module.clone(),
        json!({"module": module, "updated_at_ms": now_ms_cli()}),
    );
    if let Err(error) = write_json_file(&path, &config) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"registered": true, "path": path.to_string_lossy(), "module": module}),
    )
}

fn github_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent github <status|issue|pr> [args...]");
    }
    match args[0].as_str() {
        "status" => run_external_json("gh", &["status"]),
        "issue" => run_external_json("gh", &["issue", "list", "--limit", "20"]),
        "pr" => run_external_json("gh", &["pr", "list", "--limit", "20"]),
        other => err_text(2, format!("unknown github command: {other}")),
    }
}

fn pr_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent pr <number>");
    }
    let Some(number) = args.first() else {
        return err_text(2, "pr requires a number");
    };
    run_external_json("gh", &["pr", "checkout", number])
}

fn debug_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent debug <info|paths|sessions|file|rg>");
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
        "sessions" => session_list(&args[1..]),
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
        return ok_text("Usage: openagent db <path|summary>");
    }
    match args.first().map(String::as_str).unwrap_or("summary") {
        "path" => ok_text(
            session_root_from_args(args)
                .join("index.jsonl")
                .to_string_lossy(),
        ),
        "summary" => stats_command(args),
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
        "reason": "OpenAgent is source-tree managed in this workspace; package lifecycle is a distribution concern.",
    }))
}

fn acp_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent acp [--host <host>] [--port <port>] [--cwd <path>]");
    }
    let mut runtime_args = args.to_vec();
    runtime_args.push("--headless".to_string());
    http_runtime_command(&runtime_args, false, "Usage: openagent acp")
}

fn generate_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent generate");
    }
    CliRunResult::ok_json(&json!({
        "openapi": "3.1.0",
        "info": {"title": "OpenAgent App Bridge", "version": env!("CARGO_PKG_VERSION")},
        "paths": {
            "/api/health": {"get": {"operationId": "health"}},
            "/api/sessions": {"get": {"operationId": "listSessions"}, "post": {"operationId": "createSession"}},
            "/api/sessions/{session_id}/turns": {"post": {"operationId": "startTurn"}}
        }
    }))
}

fn console_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent console <login|logout|orgs|open>");
    }
    let path = home_dir().join(".config/openagent/console.json");
    match args[0].as_str() {
        "login" => {
            let url = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| "https://app.openagent.local".to_string());
            let payload = json!({"url": url, "updated_at_ms": now_ms_cli()});
            if let Err(error) = write_json_file(&path, &payload) {
                return err_text(1, error);
            }
            CliRunResult::ok_json(&json!({"logged_in": true, "path": path.to_string_lossy()}))
        }
        "logout" => {
            let removed = fs::remove_file(&path).is_ok();
            CliRunResult::ok_json(&json!({"logged_out": removed}))
        }
        "orgs" | "open" => CliRunResult::ok_json(&read_json_file(&path)),
        other => err_text(2, format!("unknown console command: {other}")),
    }
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
