use super::*;

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
fn binary_run_discovers_and_executes_stdio_mcp_tool() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-stdio-mcp-loop")?;
    let session_root = temp.join("sessions");
    let mcp_config = temp.join("mcp.json");
    let server_script = temp.join("stdio_mcp_server.py");
    fs::write(&server_script, stdio_mcp_server_script())?;
    fs::write(
        &mcp_config,
        format!(
            r#"{{
              "mcpServers": {{
                "arbor-review": {{
                  "command": "python3",
                  "args": ["{}"],
                  "enabled": true,
                  "timeout_ms": 5000
                }}
              }}
            }}"#,
            server_script.display()
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
            r#"[{"call_id":"call_mcp","name":"mcp_tool_arbor_review_arbor_review","input":{"text":"hi"}}]"#,
        )
        .env("OPENAGENT_MOCK_ANSWER", "stdio mcp complete")
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
    let completed = events
        .iter()
        .find(|event| event["method"] == "item/toolCall/completed")
        .ok_or("missing stdio mcp tool completion")?;
    assert_eq!(
        completed["params"]["name"],
        "mcp_tool_arbor_review_arbor_review"
    );
    assert_eq!(completed["params"]["output"], "stdio MCP echo hi");
    assert_eq!(completed["params"]["metadata"]["backend"], "mcp");
    assert_eq!(completed["params"]["metadata"]["mcp_transport"], "stdio");
    assert!(events.iter().any(|event| {
        event["method"] == "turn/completed"
            && event["params"]["final_answer"] == "stdio mcp complete"
    }));

    let _ = fs::remove_dir_all(temp);
    Ok(())
}
