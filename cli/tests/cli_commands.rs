use std::{
    error::Error,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
    time::{SystemTime, UNIX_EPOCH},
};

use openagent_cli::cli_commands_fixture;
use serde_json::{Value, json};

type MockServer = thread::JoinHandle<Result<(), String>>;

#[test]
fn cli_commands_fixture_matches_legacy_oracle() -> Result<(), Box<dyn Error>> {
    let fixture = read_fixture()?;
    assert_eq!(cli_commands_fixture(), fixture);
    Ok(())
}

#[test]
fn binary_default_smoke_prints_command_name() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_openagent")).output()?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout)?, "openagent\n");
    assert_eq!(String::from_utf8(output.stderr)?, "");
    Ok(())
}

#[test]
fn binary_doctor_json_smoke_uses_environment() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args(["doctor", "--format", "json"])
        .env_clear()
        .env("OPENAI_API_KEY", "secret")
        .env("OPENAI_BASE_URL", "http://gateway.test")
        .env("OPENAI_MODEL", "gpt-test")
        .env("OPENAI_WIRE_API", "responses")
        .env("OPENAGENT_DOCTOR_MODEL_ENDPOINT_OK", "1")
        .env(
            "OPENAGENT_DOCTOR_MODEL_ENDPOINT_MESSAGE",
            "http://gateway.test/v1/models",
        )
        .output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    let payload: Value = serde_json::from_str(&stdout)?;
    assert_eq!(payload["provider"], "openai");
    assert_eq!(payload["base_url"], "http://gateway.test");
    assert_eq!(payload["model_endpoint_ok"], true);
    assert!(!stdout.contains("secret"));
    Ok(())
}

#[test]
fn binary_doctor_json_respects_cli_model_overrides() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "doctor",
            "--format",
            "json",
            "--base-url",
            "http://cli.test",
            "--model",
            "gpt-cli",
            "--wire-api",
            "chat",
            "--api-key",
            "cli-secret",
        ])
        .env_clear()
        .env("OPENAI_BASE_URL", "http://env.test")
        .env("OPENAI_MODEL", "gpt-env")
        .env("OPENAI_WIRE_API", "responses")
        .env("OPENAGENT_DOCTOR_MODEL_ENDPOINT_OK", "1")
        .env("OPENAGENT_DOCTOR_MODEL_ENDPOINT_MESSAGE", "cli probe")
        .output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    let payload: Value = serde_json::from_str(&stdout)?;
    assert_eq!(payload["base_url"], "http://cli.test");
    assert_eq!(payload["model"], "gpt-cli");
    assert_eq!(payload["wire_api"], "chat");
    assert_eq!(payload["api_key_set"], true);
    assert!(!stdout.contains("cli-secret"));
    Ok(())
}

#[test]
fn binary_help_smoke_covers_legacy_command_surface() -> Result<(), Box<dyn Error>> {
    let root = run_openagent(["--help"], None)?;
    assert!(root.status.success());
    let root_stdout = String::from_utf8(root.stdout)?;
    for command in [
        "tui",
        "run",
        "serve",
        "web",
        "client",
        "attach",
        "session",
        "models",
        "stats",
        "command",
        "config",
        "auth",
        "providers",
        "mcp",
        "doctor",
    ] {
        assert!(
            root_stdout.contains(command),
            "root help should mention {command}"
        );
        let output = run_openagent([command, "--help"], None)?;
        assert!(
            output.status.success(),
            "{command} --help failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let run_help = run_openagent(["run", "--help"], None)?;
    let run_stdout = String::from_utf8(run_help.stdout)?;
    for opencode_flag in [
        "--fork",
        "--share",
        "--agent",
        "--title",
        "--attach",
        "--variant",
        "--thinking",
        "--dangerously-skip-permissions",
    ] {
        assert!(
            run_stdout.contains(opencode_flag),
            "run help should expose OpenCode parity flag {opencode_flag}"
        );
    }
    Ok(())
}

#[test]
fn binary_run_and_models_smokes_are_machine_readable() -> Result<(), Box<dyn Error>> {
    let run = run_openagent(
        ["run", "--skip-doctor", "--format", "json", "hello", "agent"],
        None,
    )?;
    assert!(run.status.success());
    let events = String::from_utf8(run.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(events[0]["method"], "item/agentMessage/delta");
    assert_eq!(events[1]["method"], "turn/completed");

    let models = run_openagent(["models", "--format", "json"], None)?;
    assert!(models.status.success());
    let payload: Value = serde_json::from_slice(&models.stdout)?;
    assert_eq!(payload["provider"], "openai");
    assert_eq!(payload["models"][0]["id"], "gpt-5.5");
    Ok(())
}

#[test]
fn binary_run_does_not_leak_flag_values_into_prompt() -> Result<(), Box<dyn Error>> {
    let run = run_openagent(
        [
            "run",
            "--skip-doctor",
            "--base-url",
            "http://private-gateway.test",
            "--model",
            "gpt-private",
            "--api-key",
            "private-key",
            "--max-steps",
            "2",
            "--format",
            "json",
            "hello",
        ],
        None,
    )?;
    assert!(run.status.success());
    let stdout = String::from_utf8(run.stdout)?;
    let events = stdout
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(events[0]["params"]["prompt"], "hello");
    assert!(!stdout.contains("private-gateway"));
    assert!(!stdout.contains("gpt-private"));
    assert!(!stdout.contains("private-key"));
    Ok(())
}

#[test]
fn binary_models_uses_provider_specific_model_environment() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args(["models", "anthropic", "--format", "json"])
        .env_clear()
        .env("OPENAI_MODEL", "gpt-env")
        .env("ANTHROPIC_MODEL", "claude-env")
        .output()?;
    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(payload["provider"], "anthropic");
    assert_eq!(payload["models"][0]["id"], "claude-env");
    Ok(())
}

#[test]
fn binary_config_auth_and_mcp_file_flows_work_without_python() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-flow")?;
    let config_path = temp.join("openagent.env");
    let auth_path = temp.join("auth.json");
    let mcp_path = temp.join("mcp.json");

    let config = run_openagent(
        [
            "config",
            "init",
            "--path",
            path_str(&config_path),
            "--api-key",
            "secret-key",
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(config.status.success());
    let config_payload: Value = serde_json::from_slice(&config.stdout)?;
    assert_eq!(config_payload["created"], true);
    assert!(fs::read_to_string(&config_path)?.contains("OPENAI_API_KEY=secret-key"));

    let login = run_openagent(
        [
            "auth",
            "login",
            "--auth-file",
            path_str(&auth_path),
            "--provider",
            "groq",
            "--api-key",
            "groq-secret",
            "--base-url",
            "https://api.groq.example/v1",
            "--model",
            "llama-fixture",
        ],
        None,
    )?;
    assert!(login.status.success());
    let login_payload: Value = serde_json::from_slice(&login.stdout)?;
    assert_eq!(login_payload["status"], "logged_in");
    assert!(!String::from_utf8(login.stdout)?.contains("groq-secret"));

    let list = run_openagent(
        [
            "providers",
            "list",
            "--auth-file",
            path_str(&auth_path),
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(list.status.success());
    let list_payload: Value = serde_json::from_slice(&list.stdout)?;
    assert_eq!(list_payload["providers"][0]["provider"], "groq");

    let mcp_add = run_openagent(
        [
            "mcp",
            "add",
            "demo",
            "--config",
            path_str(&mcp_path),
            "--url",
            "https://user:password@example.com/mcp?token=secret&safe=1",
            "--header",
            "Authorization=Bearer private",
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(mcp_add.status.success());
    let add_payload: Value = serde_json::from_slice(&mcp_add.stdout)?;
    assert_eq!(add_payload["updated"], true);
    let add_stdout = String::from_utf8(mcp_add.stdout)?;
    assert!(!add_stdout.contains("password"));
    assert!(!add_stdout.contains("secret&"));
    assert!(!add_stdout.contains("Bearer private"));

    let doctor = run_openagent(
        [
            "mcp",
            "doctor",
            "--config",
            path_str(&mcp_path),
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(doctor.status.success());
    let doctor_payload: Value = serde_json::from_slice(&doctor.stdout)?;
    assert_eq!(doctor_payload["server_count"], 1);

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_models_catalog_and_backlog_commands_are_deep_local_workflows()
-> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-deep-workflows")?;
    let models_cache = temp.join("models.json");
    let models_body = r#"{
      "openai": {
        "id": "openai",
        "name": "OpenAI",
        "api": "https://api.openai.com/v1",
        "doc": "https://platform.openai.com/docs",
        "env": ["OPENAI_API_KEY"],
        "models": {
          "openai/gpt-test": {
            "id": "openai/gpt-test",
            "name": "GPT Test",
            "family": "gpt",
            "attachment": true,
            "reasoning": true,
            "tool_call": true,
            "structured_output": true,
            "modalities": {"input": ["text", "image"], "output": ["text"]},
            "limit": {"context": 128000, "output": 16384},
            "cost": {"input": 1.25, "output": 10, "cache_read": 0.125}
          }
        }
      },
      "google": {
        "id": "google",
        "name": "Google",
        "models": {
          "google/gemini-test": {
            "id": "google/gemini-test",
            "name": "Gemini Test",
            "reasoning": true,
            "tool_call": true,
            "modalities": {"input": ["text", "image", "pdf"], "output": ["text"]},
            "limit": {"context": 1048576, "output": 65536},
            "cost": {"input": 2, "output": 12}
          }
        }
      }
    }"#;
    let (port, server) = serve_http_once_on_free_port("application/json", models_body.to_string())?;
    let models = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "models",
            "openai",
            "--refresh",
            "--models-url",
            &format!("http://127.0.0.1:{port}"),
            "--ttl-seconds",
            "3600",
            "--format",
            "json",
        ])
        .env_clear()
        .env("OPENAGENT_MODELS_PATH", path_str(&models_cache))
        .output()?;
    assert!(
        models.status.success(),
        "{}",
        String::from_utf8_lossy(&models.stderr)
    );
    server
        .join()
        .expect("models server thread")
        .expect("models response");
    let payload: Value = serde_json::from_slice(&models.stdout)?;
    assert_eq!(payload["cache"]["status"], "refreshed");
    assert_eq!(payload["models"][0]["id"], "openai/gpt-test");
    assert_eq!(payload["models"][0]["provider_model_id"], "gpt-test");
    assert_eq!(payload["models"][0]["capabilities"]["vision"], true);

    let catalog = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args(["models", "--catalog", "--offline", "--format", "json"])
        .env_clear()
        .env("OPENAGENT_MODELS_PATH", path_str(&models_cache))
        .output()?;
    assert!(catalog.status.success());
    let catalog_payload: Value = serde_json::from_slice(&catalog.stdout)?;
    assert!(
        catalog_payload["providers"]
            .as_array()
            .is_some_and(|items| { items.iter().any(|item| item["id"] == "gemini") })
    );

    fs::remove_file(&models_cache)?;
    let fallback = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "models",
            "openai",
            "--refresh",
            "--models-url",
            "http://127.0.0.1:9",
            "--format",
            "json",
        ])
        .env_clear()
        .env("OPENAGENT_MODELS_PATH", path_str(&models_cache))
        .output()?;
    assert!(fallback.status.success());
    let fallback_payload: Value = serde_json::from_slice(&fallback.stdout)?;
    assert_eq!(fallback_payload["fallback"], true);
    assert_eq!(fallback_payload["cache"]["status"], "snapshot_fallback");

    let plugin_dir = temp.join("demo-plugin");
    fs::create_dir_all(plugin_dir.join(".codex-plugin"))?;
    fs::write(
        plugin_dir.join(".codex-plugin/plugin.json"),
        r#"{"id":"demo-plugin","name":"Demo Plugin","commands":{"default":{"description":"demo"}}}"#,
    )?;
    let plugin = run_openagent(
        [
            "plugin",
            "install",
            path_str(&plugin_dir),
            "--workspace",
            path_str(&temp),
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(plugin.status.success());
    let plugin_payload: Value = serde_json::from_slice(&plugin.stdout)?;
    assert_eq!(plugin_payload["plugin_id"], "demo-plugin");

    let workflow = run_openagent(
        [
            "github",
            "workflow",
            "123",
            "--workspace",
            path_str(&temp),
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(workflow.status.success());
    let workflow_payload: Value = serde_json::from_slice(&workflow.stdout)?;
    assert_eq!(workflow_payload["workflow"]["branch"], "openagent/123");

    let session_root = temp.join("sessions");
    fs::create_dir_all(session_root.join("s1/runs/r1"))?;
    fs::write(
        session_root.join("s1/state.latest.json"),
        r#"{"session_id":"s1","workspace":"alpha-workspace","status":"idle","updated_at_ms":10,"messages":[{"role":"user","content":"hi"}]}"#,
    )?;
    let db = run_openagent(
        [
            "db",
            "rebuild",
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(db.status.success());
    let db_payload: Value = serde_json::from_slice(&db.stdout)?;
    assert_eq!(db_payload["rows"], 1);
    let query = run_openagent(
        [
            "db",
            "query",
            "alpha",
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(query.status.success());
    let query_payload: Value = serde_json::from_slice(&query.stdout)?;
    assert_eq!(query_payload["rows"].as_array().map_or(0, Vec::len), 1);

    let generate = run_openagent(["generate", "commands"], None)?;
    assert!(generate.status.success());
    let generate_payload: Value = serde_json::from_slice(&generate.stdout)?;
    assert!(
        generate_payload["commands"]
            .as_array()
            .is_some_and(|items| { items.iter().any(|item| item == "plugin") })
    );

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_run_streams_openai_chat_sse_provider_events() -> Result<(), Box<dyn Error>> {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"streamed\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n"
    )
    .to_string();
    let (port, server) = serve_http_once_on_free_port("text/event-stream", body)?;
    let output = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--provider",
            "openai",
            "--api-key",
            "secret",
            "--base-url",
            &format!("http://127.0.0.1:{port}"),
            "--wire-api",
            "chat",
            "--stream",
            "--format",
            "json",
            "hello",
        ])
        .env_clear()
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server
        .join()
        .expect("provider server thread")
        .expect("provider response");
    let events = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(events.iter().any(|event| {
        event["method"] == "item/agentMessage/delta"
            && event["params"]["delta"]
                .as_str()
                .is_some_and(|text| text.contains("hello ") || text.contains("streamed"))
    }));
    assert!(events.iter().any(|event| {
        event["method"] == "turn/completed" && event["params"]["source"] == "openai:chat:stream"
    }));
    Ok(())
}

#[test]
fn binary_run_emits_provider_sse_delta_before_stream_closes() -> Result<(), Box<dyn Error>> {
    let (port, server) = serve_dripping_sse_provider()?;
    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--provider",
            "openai",
            "--api-key",
            "secret",
            "--base-url",
            &format!("http://127.0.0.1:{port}"),
            "--wire-api",
            "chat",
            "--stream",
            "--format",
            "json",
            "hello",
        ])
        .env_clear()
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let mut reader = BufReader::new(stdout);
    let mut first_line = String::new();
    reader.read_line(&mut first_line)?;
    assert!(
        start.elapsed() < Duration::from_millis(900),
        "first stream event should arrive before the mock server closes"
    );
    let first_event: Value = serde_json::from_str(first_line.trim())?;
    assert_eq!(first_event["method"], "item/agentMessage/delta");
    assert_eq!(first_event["params"]["delta"], "hello ");

    let mut rest = String::new();
    reader.read_to_string(&mut rest)?;
    let status = child.wait()?;
    assert!(status.success());
    server
        .join()
        .expect("provider server thread")
        .expect("provider response");
    let events = rest
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(events.iter().any(|event| {
        event["method"] == "item/agentMessage/delta" && event["params"]["delta"] == "streamed"
    }));
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "turn/completed")
    );
    Ok(())
}

#[test]
fn binary_run_executes_mock_tool_loop() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-agent-loop")?;
    fs::write(temp.join("notes.txt"), "alpha\nbeta\n")?;
    let session_root = temp.join("sessions");
    let output = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "read",
            "notes",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_read","name":"read","input":{"file_path":"notes.txt"}}]"#,
        )
        .env("OPENAGENT_MOCK_ANSWER", "final answer")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "item/toolCall/started")
    );
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "item/toolCall/completed")
    );
    let completed = events
        .iter()
        .find(|event| event["method"] == "turn/completed")
        .expect("completion event");
    assert_eq!(completed["params"]["final_answer"], "final answer");
    assert_eq!(completed["params"]["steps"], 2);
    assert_eq!(completed["params"]["tool_calls"], 1);

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_run_discovers_and_executes_remote_mcp_tool() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-mcp-loop")?;
    let session_root = temp.join("sessions");
    let mcp_config = temp.join("mcp.json");
    let (port, server) = serve_mcp_json_rpc(2)?;
    fs::write(
        &mcp_config,
        format!(
            r#"{{
              "mcp": {{
                "demo": {{
                  "type": "remote",
                  "transport": "http",
                  "url": "http://127.0.0.1:{port}",
                  "enabled": true
                }}
              }}
            }}"#
        ),
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--mcp-config",
            path_str(&mcp_config),
            "--format",
            "json",
            "call",
            "mcp",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_mcp","name":"mcp_tool_demo_echo","input":{"text":"hi"}}]"#,
        )
        .env("OPENAGENT_MOCK_ANSWER", "mcp complete")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server
        .join()
        .expect("mcp server thread")
        .expect("mcp responses");
    let events = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let completed = events
        .iter()
        .find(|event| event["method"] == "item/toolCall/completed")
        .ok_or("missing mcp tool completion")?;
    assert_eq!(completed["params"]["name"], "mcp_tool_demo_echo");
    assert_eq!(completed["params"]["output"], "MCP echo hi");
    assert_eq!(completed["params"]["metadata"]["backend"], "mcp");
    assert!(events.iter().any(|event| {
        event["method"] == "turn/completed" && event["params"]["final_answer"] == "mcp complete"
    }));

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_run_command_and_agent_profile_affect_real_run_state() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-command-agent")?;
    let command_dir = temp.join(".openagent/commands");
    fs::create_dir_all(&command_dir)?;
    fs::write(
        command_dir.join("summarize.md"),
        "Summarize this request: $ARGUMENTS",
    )?;
    let agent_create = run_openagent(
        [
            "agent",
            "create",
            "reviewer",
            "--workspace",
            path_str(&temp),
            "--provider",
            "openai",
            "--model",
            "openai/gpt-agent",
            "--permission",
            "READONLY",
            "--prompt",
            "You are a careful reviewer.",
            "--tool",
            "read",
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(
        agent_create.status.success(),
        "{}",
        String::from_utf8_lossy(&agent_create.stderr)
    );
    let session_root = temp.join("sessions");
    let run = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--agent",
            "reviewer",
            "--command",
            "summarize",
            "--format",
            "json",
            "alpha",
            "beta",
        ])
        .env_clear()
        .env("OPENAGENT_MOCK_ANSWER", "profile complete")
        .output()?;
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let events = String::from_utf8(run.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        events[0]["params"]["prompt"],
        "Summarize this request: alpha beta"
    );
    let completed = events
        .iter()
        .find(|event| event["method"] == "turn/completed")
        .ok_or("missing completion")?;
    let session_id = completed["params"]["session_id"]
        .as_str()
        .ok_or("missing session id")?;
    let state: Value = serde_json::from_str(&fs::read_to_string(
        session_root.join(session_id).join("state.latest.json"),
    )?)?;
    assert_eq!(state["metadata"]["model"], "gpt-agent");
    assert_eq!(state["metadata"]["permission"], "READONLY");
    assert_eq!(state["metadata"]["agent_profile"]["id"], "reviewer");
    assert!(state["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["role"] == "system"
                && message["content"] == "You are a careful reviewer."
                && message["metadata"]["agent_profile"] == "reviewer"
        })
    }));

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_run_queues_approval_for_dangerous_tool() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-agent-approval")?;
    let session_root = temp.join("sessions");
    let output = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "run",
            "a",
            "command",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_bash","name":"bash","input":{"command":"echo hi"}}]"#,
        )
        .output()?;
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    let events = stdout
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "turn/approval_requested")
    );
    let approval = events
        .iter()
        .find(|event| event["method"] == "turn/approval_requested")
        .expect("approval event");
    assert_eq!(approval["params"]["approval"]["tool_name"], "bash");
    assert_eq!(
        approval["params"]["approval"]["reason"],
        "permission_required"
    );
    let completed = events
        .iter()
        .find(|event| event["method"] == "turn/completed")
        .expect("failed completion event");
    assert_eq!(completed["params"]["status"], "paused");

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_approval_and_question_responses_resume_paused_runs() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-resume-queues")?;
    let session_root = temp.join("sessions");

    let approval_pause = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "run",
            "approval",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_bash","name":"bash","input":{"command":"printf approved"}}]"#,
        )
        .output()?;
    assert!(!approval_pause.status.success());
    let approval_events = String::from_utf8(approval_pause.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let approval = approval_events
        .iter()
        .find(|event| event["method"] == "turn/approval_requested")
        .ok_or("missing approval request")?;
    let approval_session = approval["params"]["session_id"]
        .as_str()
        .unwrap_or_default();
    let approval_response = run_openagent(
        [
            "approval",
            "respond",
            "--session-root",
            path_str(&session_root),
            "--session",
            approval_session,
            "--decision",
            "allow_once",
        ],
        None,
    )?;
    assert!(
        approval_response.status.success(),
        "{}",
        String::from_utf8_lossy(&approval_response.stderr)
    );
    let approval_resume = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--continue",
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
        ])
        .env_clear()
        .env("OPENAGENT_MOCK_ANSWER", "approval complete")
        .output()?;
    assert!(
        approval_resume.status.success(),
        "{}",
        String::from_utf8_lossy(&approval_resume.stderr)
    );
    let approval_resume_events = String::from_utf8(approval_resume.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(approval_resume_events.iter().any(|event| {
        event["method"] == "item/toolCall/completed" && event["params"]["output"] == "approved"
    }));
    assert!(approval_resume_events.iter().any(|event| {
        event["method"] == "turn/completed"
            && event["params"]["final_answer"] == "approval complete"
    }));

    let question_pause = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "ask",
            "question",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_question","name":"question","input":{"questions":[{"question":"Pick a mode","header":"Mode","options":[{"label":"Fast","description":"Use fast path"}]}]}}]"#,
        )
        .output()?;
    assert!(!question_pause.status.success());
    let question_events = String::from_utf8(question_pause.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let question = question_events
        .iter()
        .find(|event| event["method"] == "turn/question_requested")
        .ok_or("missing question request")?;
    let question_session = question["params"]["session_id"]
        .as_str()
        .unwrap_or_default();
    let question_response = run_openagent(
        [
            "question",
            "reply",
            "--session-root",
            path_str(&session_root),
            "--session",
            question_session,
            "--answer",
            "Fast",
        ],
        None,
    )?;
    assert!(
        question_response.status.success(),
        "{}",
        String::from_utf8_lossy(&question_response.stderr)
    );
    let question_resume = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--continue",
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
        ])
        .env_clear()
        .env("OPENAGENT_MOCK_ANSWER", "question complete")
        .output()?;
    assert!(
        question_resume.status.success(),
        "{}",
        String::from_utf8_lossy(&question_resume.stderr)
    );
    let question_resume_events = String::from_utf8(question_resume.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(question_resume_events.iter().any(|event| {
        event["method"] == "item/toolCall/completed"
            && event["params"]["output"]
                .as_str()
                .is_some_and(|text| text.contains("\"Pick a mode\"=\"Fast\""))
    }));
    assert!(question_resume_events.iter().any(|event| {
        event["method"] == "turn/completed"
            && event["params"]["final_answer"] == "question complete"
    }));

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_run_skip_permissions_auto_allows_ask_but_not_deny() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-permission-skip")?;
    let session_root = temp.join("sessions");
    let allowed = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--dangerously-skip-permissions",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "run",
            "a",
            "command",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_bash","name":"bash","input":{"command":"printf allowed"}}]"#,
        )
        .env("OPENAGENT_MOCK_ANSWER", "done")
        .output()?;
    assert!(
        allowed.status.success(),
        "{}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    let allowed_events = String::from_utf8(allowed.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        allowed_events
            .iter()
            .any(|event| event["method"] == "item/toolCall/completed"
                && event["params"]["output"] == "allowed")
    );

    let denied_path = temp.join("denied.txt");
    let denied = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--permission",
            "READONLY",
            "--dangerously-skip-permissions",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "write",
            "a",
            "file",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_write","name":"write","input":{"file_path":"denied.txt","content":"nope"}}]"#,
        )
        .env("OPENAGENT_MOCK_ANSWER", "blocked")
        .output()?;
    assert!(
        denied.status.success(),
        "{}",
        String::from_utf8_lossy(&denied.stderr)
    );
    let denied_events = String::from_utf8(denied.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let failed = denied_events
        .iter()
        .find(|event| event["method"] == "item/toolCall/failed")
        .expect("denied tool failure event");
    assert_eq!(failed["params"]["metadata"]["permission_action"], "deny");
    assert_eq!(
        failed["params"]["metadata"]["error_kind"],
        "permission_denied"
    );
    assert!(!denied_path.exists());

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_attach_and_tui_attach_use_remote_bridge_events() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-cli-attach")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_openagent_server(port, &workspace, &session_root)?;
    wait_for_attach(port)?;

    let url = format!("http://127.0.0.1:{port}");
    let run = run_openagent_vec(vec![
        "run".to_string(),
        "--attach".to_string(),
        url.clone(),
        "--server-token".to_string(),
        "secret".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "hello".to_string(),
        "attach".to_string(),
    ])?;
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let events = String::from_utf8(run.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "item/agentMessage/delta")
    );
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "turn/completed")
    );

    let attach = run_openagent_vec(vec![
        "attach".to_string(),
        url.clone(),
        "--server-token".to_string(),
        "secret".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ])?;
    assert!(attach.status.success());
    let payload: Value = serde_json::from_slice(&attach.stdout)?;
    assert_eq!(payload["attached"], true);
    assert!(
        payload["sessions"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    let tui_attach = run_openagent_vec(vec![
        "tui".to_string(),
        "--attach".to_string(),
        url,
        "--server-token".to_string(),
        "secret".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ])?;
    assert!(tui_attach.status.success());
    let payload: Value = serde_json::from_slice(&tui_attach.stdout)?;
    assert_eq!(payload["attached"], true);

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

fn read_fixture() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/golden/rust_rewrite/cli_commands.json");
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn run_openagent<const N: usize>(
    args: [&str; N],
    cwd: Option<&Path>,
) -> Result<std::process::Output, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_openagent"));
    command.args(args).env_clear();
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    Ok(command.output()?)
}

fn run_openagent_vec(args: Vec<String>) -> Result<std::process::Output, Box<dyn Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args(args)
        .env_clear()
        .output()?)
}

fn spawn_openagent_server(
    port: u16,
    workspace: &Path,
    session_root: &Path,
) -> Result<Child, Box<dyn Error>> {
    let port = port.to_string();
    Ok(Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            &port,
            "--workspace",
            path_str(workspace),
            "--session-root",
            path_str(session_root),
            "--auth-token",
            "secret",
        ])
        .env_clear()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?)
}

fn wait_for_attach(port: u16) -> Result<(), Box<dyn Error>> {
    let url = format!("http://127.0.0.1:{port}");
    for _ in 0..50 {
        let output = run_openagent_vec(vec![
            "attach".to_string(),
            url.clone(),
            "--server-token".to_string(),
            "secret".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ])?;
        if output.status.success() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err("server did not accept attach".into())
}

fn free_port() -> Result<u16, Box<dyn Error>> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

fn serve_http_once_on_free_port(
    content_type: &str,
    body: String,
) -> Result<(u16, MockServer), Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    Ok((
        port,
        serve_http_once_with_listener(listener, content_type, body),
    ))
}

fn serve_http_once_with_listener(
    listener: TcpListener,
    content_type: &str,
    body: String,
) -> MockServer {
    let content_type = content_type.to_string();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
        let _ = read_http_request_body(&mut stream)?;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .map_err(|error| error.to_string())
    })
}

fn serve_dripping_sse_provider() -> Result<(u16, MockServer), Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
        let _ = read_http_request_body(&mut stream)?;
        let first =
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hello \"},\"finish_reason\":null}]}\n\n";
        let second =
            b"data: {\"choices\":[{\"delta\":{\"content\":\"streamed\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n";
        let done = b"data: [DONE]\n\n";
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
            )
            .map_err(|error| error.to_string())?;
        write_http_chunk(&mut stream, first)?;
        stream.flush().map_err(|error| error.to_string())?;
        thread::sleep(Duration::from_millis(500));
        write_http_chunk(&mut stream, second)?;
        write_http_chunk(&mut stream, done)?;
        stream
            .write_all(b"0\r\n\r\n")
            .map_err(|error| error.to_string())?;
        stream.flush().map_err(|error| error.to_string())
    });
    Ok((port, server))
}

fn write_http_chunk(stream: &mut std::net::TcpStream, chunk: &[u8]) -> Result<(), String> {
    stream
        .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
        .map_err(|error| error.to_string())?;
    stream.write_all(chunk).map_err(|error| error.to_string())?;
    stream.write_all(b"\r\n").map_err(|error| error.to_string())
}

fn serve_mcp_json_rpc(expected_requests: usize) -> Result<(u16, MockServer), Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let server = thread::spawn(move || {
        for _ in 0..expected_requests {
            let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
            let body = read_http_request_body(&mut stream)?;
            let request: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
            let method = request.get("method").and_then(Value::as_str).unwrap_or("");
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let response = if method == "tools/list" {
                json_response_body(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [{
                            "name": "echo",
                            "title": "Echo",
                            "description": "Echo text",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"text": {"type": "string"}},
                                "required": ["text"]
                            }
                        }]
                    }
                }))
            } else if method == "tools/call" {
                let text = request
                    .get("params")
                    .and_then(|params| params.get("arguments"))
                    .and_then(|arguments| arguments.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                json_response_body(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": format!("MCP echo {text}")}],
                        "isError": false
                    }
                }))
            } else {
                json_response_body(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": "method not found"}
                }))
            };
            stream
                .write_all(response.as_bytes())
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    });
    Ok((port, server))
}

fn read_http_request_body(stream: &mut std::net::TcpStream) -> Result<String, String> {
    let mut buffer = [0_u8; 8192];
    let read = stream
        .read(&mut buffer)
        .map_err(|error| error.to_string())?;
    let raw = String::from_utf8_lossy(&buffer[..read]).to_string();
    let (headers, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "invalid HTTP request".to_string())?;
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(body.len());
    let mut body = body.as_bytes().to_vec();
    while body.len() < content_length {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    body.truncate(content_length);
    String::from_utf8(body).map_err(|error| error.to_string())
}

fn json_response_body(value: Value) -> String {
    let body = value.to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn temp_dir(prefix: &str) -> Result<PathBuf, Box<dyn Error>> {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    let path = std::env::temp_dir().join(format!("{prefix}-{suffix}"));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap_or("")
}
