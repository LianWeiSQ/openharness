#[test]
fn binary_agent_registry_exposes_builtin_subagents() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-builtin-agents")?;
    let list = run_openagent(
        [
            "agent",
            "list",
            "--workspace",
            path_str(&temp),
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let payload: Value = serde_json::from_slice(&list.stdout)?;
    let agents = payload["agents"].as_array().ok_or("missing agents")?;
    for id in ["build", "general", "explore", "plan"] {
        assert!(agents.iter().any(|agent| agent["id"] == id), "missing {id}");
    }

    let show = run_openagent(
        [
            "agent",
            "show",
            "explore",
            "--workspace",
            path_str(&temp),
            "--format",
            "json",
        ],
        None,
    )?;
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let explore: Value = serde_json::from_slice(&show.stdout)?;
    assert_eq!(explore["id"], "explore");
    assert_eq!(explore["mode"], "subagent");
    assert_eq!(explore["permission"], "READONLY");
    assert!(
        explore["description"]
            .as_str()
            .is_some_and(|value| value.contains("Read-only"))
    );

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_run_executes_task_subagent_tool() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-task-subagent")?;
    let session_root = temp.join("sessions");
    let output = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--permission",
            "FULL",
            "--format",
            "json",
            "delegate",
            "this",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_task","name":"task","input":{"description":"Explore fixture","prompt":"Find the important files and summarize them.","subagent_type":"explore"}}]"#,
        )
        .env("OPENAGENT_MOCK_SUBAGENT_ANSWER", "child answer")
        .env("OPENAGENT_MOCK_ANSWER", "parent final")
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
    let tool_completed = events
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing task completion")?;
    assert_eq!(
        tool_completed["params"]["metadata"]["subagent_type"],
        "explore"
    );
    assert!(
        tool_completed["params"]["output"]
            .as_str()
            .is_some_and(|output| output.contains("<task id=") && output.contains("child answer"))
    );
    let child_session_id = tool_completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?;
    let completed = events
        .iter()
        .find(|event| event["method"] == "turn/completed")
        .ok_or("missing completion event")?;
    assert_eq!(completed["params"]["final_answer"], "parent final");
    assert_eq!(completed["params"]["steps"], 2);
    assert_eq!(completed["params"]["tool_calls"], 1);
    let parent_session_id = completed["params"]["session_id"]
        .as_str()
        .ok_or("missing parent session id")?;

    let child_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(child_session_id)
            .join("state.latest.json"),
    )?)?;
    assert_eq!(child_state["metadata"]["subagent"], true);
    assert_eq!(
        child_state["metadata"]["parent_session_id"],
        parent_session_id
    );
    assert_eq!(child_state["metadata"]["parent_tool_call_id"], "call_task");
    assert_eq!(child_state["metadata"]["agent_profile"]["id"], "explore");
    assert!(child_state["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["role"] == "system" && message["metadata"]["agent_profile"] == "explore"
        }) && messages.iter().any(|message| {
            message["role"] == "user"
                && message["content"] == "Find the important files and summarize them."
        })
    }));

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
