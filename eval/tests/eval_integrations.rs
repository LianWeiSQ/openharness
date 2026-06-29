use std::{error::Error, fs, path::PathBuf};

use openagent_eval::{
    eval_integrations_fixture, harbor_normalized_model_name, harbor_timeout_seconds,
    terminal_bench_extract_returncode, terminal_bench_failure_mode,
};
use serde_json::Value;

#[test]
fn eval_integrations_fixture_matches_python_oracle() -> Result<(), Box<dyn Error>> {
    let fixture = read_fixture()?;
    assert_eq!(eval_integrations_fixture(), fixture);
    Ok(())
}

#[test]
fn benchmark_adapter_helpers_cover_edge_cases() {
    let (returncode, cleaned) = terminal_bench_extract_returncode(
        "body\n__OPENAGENT_TBENCH_EXIT_x__-9\n",
        "__OPENAGENT_TBENCH_EXIT_x__",
    );
    assert_eq!(returncode, -9);
    assert_eq!(cleaned, "body");
    assert_eq!(
        terminal_bench_failure_mode("context length exceeded"),
        "context_length_exceeded"
    );
    assert_eq!(harbor_timeout_seconds(5200), 6);
    assert_eq!(
        harbor_normalized_model_name(Some("openai-compatible/gpt-test")),
        Some("gpt-test".to_string())
    );
}

fn read_fixture() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/rust_rewrite/eval_integrations.json");
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}
