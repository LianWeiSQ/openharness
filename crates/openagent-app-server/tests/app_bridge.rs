use std::{error::Error, fs, path::PathBuf};

use openagent_app_server::{app_bridge_protocol_fixture, app_bridge_server_fixture};
use serde_json::Value;

#[test]
fn app_bridge_protocol_matches_python_oracle() -> Result<(), Box<dyn Error>> {
    let fixture = read_fixture()?;
    assert_eq!(app_bridge_protocol_fixture(), section(&fixture, "protocol"));
    Ok(())
}

#[test]
fn app_bridge_server_matches_python_oracle() -> Result<(), Box<dyn Error>> {
    let fixture = read_fixture()?;
    assert_eq!(app_bridge_server_fixture(), section(&fixture, "server"));
    Ok(())
}

fn read_fixture() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/rust_rewrite/app_bridge_tui.json");
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn section(fixture: &Value, name: &str) -> Value {
    fixture.get(name).cloned().unwrap_or(Value::Null)
}
