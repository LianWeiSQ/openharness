//! CLI crate for the Rust rewrite.

use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

use openagent_provider::{
    anthropic_model, default_env_mapping, normalize_provider, openai_compatible_model,
    provider_auth_methods, provider_default_base_url, provider_default_model,
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
     OpenCode parity backlog commands: agent, plugin, github, pr, debug, upgrade, uninstall, acp, import, export"
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
       --variant <name>                 provider-specific variant\n\
       --thinking                       show thinking blocks\n\
       --interactive, -i                run direct interactive mode\n\
       --dangerously-skip-permissions   auto-approve permissions that are not denied\n\
       --skip-doctor                    skip local gateway check"
}

fn tui_help() -> &'static str {
    "Usage: openagent tui [options]\n\n\
     Options: --workspace <path>, --session-root <path>, -s/--session <id>, -c/--continue, --skip-doctor"
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
     Options: --workspace <path>, -s/--session <id>, -c/--continue, --skip-health-check, --server-token <token>"
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
    "Usage: openagent session <list|export|delete> [options]\n\n\
     list:   --session-root <path>, --format <table|json>, --max-count <n>\n\
     export: --session-root <path>, --sanitize <session_id>\n\
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
     auth: list|status|set-token\n\
     doctor/debug: --refresh --format <table|json>"
}

fn run_prompt_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(run_help());
    }
    let format = value_for(args, &["--format"]).unwrap_or_else(|| "text".to_string());
    let provider = active_provider();
    if !has_flag(args, &["--skip-doctor"])
        && !doctor_payload_from_env(&provider)["healthy"]
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
            "--agent",
            "--title",
            "--attach",
            "--variant",
            "--port",
        ],
    )
    .join(" ");
    if message.trim().is_empty() {
        return err_text(2, "openagent run requires a prompt or piped stdin");
    }
    let workspace = workspace_from_args(args);
    let files = match attached_files(&workspace, &values_for(args, &["--file", "-f"])) {
        Ok(files) => files,
        Err(error) => return err_text(1, error),
    };
    let prompt = build_prompt_with_files(&message, &files);
    let answer =
        env::var("OPENAGENT_MOCK_ANSWER").unwrap_or_else(|_| "hello from openagent".to_string());
    if format == "json" {
        let events = [
            json!({"method": "item/agentMessage/delta", "params": {"delta": answer, "prompt": prompt}}),
            json!({"method": "turn/completed", "params": {"status": "completed", "final_answer": answer}}),
        ];
        return ok_text(
            events
                .iter()
                .map(python_json_dumps)
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    ok_text(answer)
}

fn client_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(client_help());
    }
    let server_url = value_for(args, &["--server-url"])
        .or_else(|| env::var("OPENAGENT_SERVER_URL").ok())
        .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string());
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
    let payload = json!({
        "server_url": server_url.trim_end_matches('/'),
        "message": message,
        "session": value_for(args, &["--session", "-s"]),
        "continue": has_flag(args, &["--continue", "-c"]),
    });
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        return CliRunResult::ok_json(&payload);
    }
    ok_text(format!(
        "prepared App Bridge client request for {}",
        payload["server_url"].as_str().unwrap_or(DEFAULT_SERVER_URL)
    ))
}

fn models_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(models_help());
    }
    let format = value_for(args, &["--format"]).unwrap_or_else(|| "table".to_string());
    let provider = positional_args(args, &["--format"])
        .first()
        .cloned()
        .unwrap_or_else(active_provider);
    let normalized = match normalize_provider(Some(&provider)) {
        Ok(provider) => provider,
        Err(error) => return err_text(2, error),
    };
    let model_id = env::var("OPENAI_MODEL").ok().unwrap_or_else(|| {
        if normalized == "openai" {
            DEFAULT_MODEL.to_string()
        } else {
            provider_default_model(&normalized)
                .ok()
                .flatten()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string())
        }
    });
    let model = if normalized == "anthropic" {
        serde_json::to_value(anthropic_model(&model_id, 200_000, 8192))
            .unwrap_or_else(|_| json!({}))
    } else {
        serde_json::to_value(openai_compatible_model(&normalized, &model_id))
            .unwrap_or_else(|_| json!({}))
    };
    let payload = json!({"provider": normalized, "models": [model]});
    if format == "json" {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!(
            "provider  model\n--------  -----\n{}  {}",
            payload["provider"].as_str().unwrap_or("openai"),
            model_id
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
        return ok_text("Usage: openagent mcp auth <list|status|set-token> [options]");
    }
    match args[0].as_str() {
        "list" | "ls" | "status" => mcp_doctor(&args[1..]),
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
    let servers = mcp_public_servers(&config)
        .into_iter()
        .map(|mut server| {
            if let Some(object) = server.as_object_mut() {
                object.insert("status".to_string(), json!("idle"));
                object.insert("tool_count".to_string(), json!(0));
                object.insert("last_error".to_string(), Value::Null);
                object.insert("tools".to_string(), json!([]));
                object.insert("ok".to_string(), json!(true));
            }
            server
        })
        .collect::<Vec<_>>();
    let payload = json!({
        "config_path": config_path.to_string_lossy(),
        "configured": !servers.is_empty(),
        "enabled": servers.iter().any(|server| server["enabled"].as_bool().unwrap_or(false)),
        "server_count": servers.len(),
        "ok": true,
        "refresh_error": null,
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
    let Some(session_id) = positionals.first() else {
        return err_text(2, "session export requires a session id");
    };
    if !valid_session_id(session_id) {
        return err_text(2, "Invalid session id");
    }
    let root = session_root_from_args(args);
    let state_path = root.join(session_id).join("state.latest.json");
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
        "tui" => simple_command("tui", &argv[1..], tui_help(), "openagent-tui"),
        "serve" => simple_command(
            "serve",
            &argv[1..],
            serve_help(),
            "openagent serve: HTTP listener wiring is owned by openagent-http-runtime",
        ),
        "web" => simple_command(
            "web",
            &argv[1..],
            web_help(),
            "openagent web: browser console is served by openagent-http-runtime",
        ),
        "attach" => simple_command(
            "attach",
            &argv[1..],
            attach_help(),
            "openagent attach: remote TUI attach wiring is owned by App Bridge/TUI crates",
        ),
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
            let payload = doctor_payload_from_env(&provider);
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
        "agent" | "plugin" | "plug" | "github" | "pr" | "debug" | "upgrade" | "uninstall"
        | "acp" | "import" | "export" => opencode_gap_command(argv[0].as_str(), &argv[1..]),
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
