use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use openagent_cli::cli_commands_fixture;
use serde_json::Value;

#[test]
fn cli_commands_fixture_matches_python_oracle() -> Result<(), Box<dyn Error>> {
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

fn read_fixture() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/rust_rewrite/cli_commands.json");
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
