use std::{error::Error, fs, path::PathBuf, process::Command};

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

fn read_fixture() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/rust_rewrite/cli_commands.json");
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}
