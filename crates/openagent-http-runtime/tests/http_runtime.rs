use std::{error::Error, fs, path::PathBuf, process::Command};

use openagent_http_runtime::http_runtime_fixture;
use serde_json::Value;

#[test]
fn http_runtime_fixture_matches_python_oracle() -> Result<(), Box<dyn Error>> {
    let fixture = read_fixture()?;
    assert_eq!(http_runtime_fixture(), fixture);
    Ok(())
}

#[test]
fn binary_health_json_smoke_matches_docker_contract() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_openagent-http-runtime"))
        .arg("--health-json")
        .output()?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        payload,
        section(&read_fixture()?, "docker")["expected_stdout_json"]
    );
    Ok(())
}

#[test]
fn dockerfile_matches_smoke_contract() -> Result<(), Box<dyn Error>> {
    let dockerfile = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Dockerfile.openagent-http-runtime"),
    )?;
    let lines = dockerfile
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(Value::from)
        .collect::<Vec<_>>();
    assert_eq!(
        Value::Array(lines),
        section(&read_fixture()?, "docker")["dockerfile"]
    );
    Ok(())
}

fn read_fixture() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/rust_rewrite/http_runtime.json");
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn section(fixture: &Value, name: &str) -> Value {
    fixture.get(name).cloned().unwrap_or(Value::Null)
}
