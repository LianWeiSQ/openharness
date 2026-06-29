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
