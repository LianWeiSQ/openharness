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
fn binary_agent_registry_loads_opencode_markdown_agents() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-opencode-agent-md")?;
    let session_root = temp.join("sessions");
    let agent_dir = temp.join(".opencode/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("markdown-research.md"),
        r#"---
id: markdown-research
name: Markdown Research
description: OpenCode markdown research agent
mode: subagent
permission: READONLY
tools:
  - read
model: markdown-child-model
steps: 2
temperature: 0.31
top_p: 0.73
reasoning_effort: medium
color: cyan
---
You are the CLI Markdown research subagent.
"#,
    )?;
    fs::write(
        agent_dir.join("disabled-worker.md"),
        r#"---
id: disabled-worker
name: Disabled Worker
mode: subagent
disable: true
---
Disabled prompt.
"#,
    )?;

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
    let markdown = agents
        .iter()
        .find(|agent| agent["id"] == "markdown-research")
        .ok_or("missing markdown agent")?;
    assert_eq!(markdown["name"], "Markdown Research");
    assert_eq!(markdown["steps"], 2);
    assert_eq!(markdown["temperature"], 0.31);
    assert_eq!(markdown["top_p"], 0.73);
    assert_eq!(markdown["color"], "cyan");
    assert_eq!(markdown["model_options"]["reasoning_effort"], "medium");
    assert!(!agents.iter().any(|agent| agent["id"] == "disabled-worker"));

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
            "markdown",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_markdown","name":"task","input":{"description":"Markdown task","prompt":"Run the markdown subagent.","subagent_type":"markdown-research"}}]"#,
        )
        .env("OPENAGENT_MOCK_SUBAGENT_ANSWER", "markdown child answer")
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
    let completed = events
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing markdown task completion")?;
    let child_session_id = completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?;
    assert_eq!(
        completed["params"]["metadata"]["model_options"]["reasoning_effort"],
        "medium"
    );
    let child_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(child_session_id)
            .join("state.latest.json"),
    )?)?;
    assert!(child_state["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["role"] == "system"
                && message["content"] == "You are the CLI Markdown research subagent."
        })
    }));
    assert_eq!(child_state["metadata"]["temperature"], 0.31);
    assert_eq!(child_state["metadata"]["top_p"], 0.73);
    assert_eq!(
        child_state["metadata"]["model_options"]["reasoning_effort"],
        "medium"
    );
    assert_eq!(child_state["metadata"]["color"], "cyan");

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
fn binary_run_enforces_agent_task_permissions() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-task-permissions")?;
    let session_root = temp.join("sessions");
    let agent_dir = temp.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("limited-build.json"),
        serde_json::to_string_pretty(&json!({
            "id": "limited-build",
            "name": "Limited Build",
            "description": "Primary agent that can only launch allowed-worker.",
            "mode": "primary",
            "permission": {
                "ruleset": "FULL",
                "task": {
                    "*": "deny",
                    "allowed-worker": "allow"
                }
            },
            "tools": ["task"]
        }))?,
    )?;
    for id in ["allowed-worker", "blocked-worker"] {
        fs::write(
            agent_dir.join(format!("{id}.json")),
            serde_json::to_string_pretty(&json!({
                "id": id,
                "name": id,
                "description": format!("{id} subagent"),
                "mode": "subagent",
                "permission": "READONLY",
                "prompt": format!("You are {id}."),
                "tools": ["read"],
                "max_steps": 2
            }))?,
        )?;
    }

    let denied = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--agent",
            "limited-build",
            "--permission",
            "FULL",
            "--format",
            "json",
            "try",
            "blocked",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_blocked","name":"task","input":{"description":"Blocked task","prompt":"Should not run.","subagent_type":"blocked-worker"}}]"#,
        )
        .env("OPENAGENT_MOCK_ANSWER", "parent handled denial")
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
        .ok_or("missing denied task failure")?;
    assert_eq!(failed["params"]["name"], "task");
    assert_eq!(failed["params"]["metadata"]["permission_action"], "deny");
    assert_eq!(
        failed["params"]["metadata"]["permission_pattern"],
        "blocked-worker"
    );
    assert!(
        !session_root.exists()
            || !fs::read_dir(&session_root)?.flatten().any(|entry| {
                let state_path = entry.path().join("state.latest.json");
                let Ok(raw) = fs::read_to_string(state_path) else {
                    return false;
                };
                let Ok(state) = serde_json::from_str::<Value>(&raw) else {
                    return false;
                };
                state["metadata"]["subagent"].as_bool().unwrap_or(false)
            })
    );

    let allowed = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--agent",
            "limited-build",
            "--permission",
            "FULL",
            "--format",
            "json",
            "try",
            "allowed",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_allowed","name":"task","input":{"description":"Allowed task","prompt":"Run allowed.","subagent_type":"allowed-worker"}}]"#,
        )
        .env("OPENAGENT_MOCK_SUBAGENT_ANSWER", "allowed child answer")
        .env("OPENAGENT_MOCK_ANSWER", "parent handled allowed")
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
    let completed = allowed_events
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing allowed task completion")?;
    assert_eq!(
        completed["params"]["metadata"]["subagent_type"],
        "allowed-worker"
    );
    assert!(
        completed["params"]["output"]
            .as_str()
            .is_some_and(|output| {
                output.contains("<task id=") && output.contains("allowed child answer")
            })
    );

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_run_invokes_subagent_with_at_mention() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-at-subagent")?;
    let session_root = temp.join("sessions");
    let agent_dir = temp.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("allowed-worker.json"),
        serde_json::to_string_pretty(&json!({
            "id": "allowed-worker",
            "name": "Allowed Worker",
            "description": "Manual at-mention worker",
            "mode": "subagent",
            "permission": "READONLY",
            "prompt": "You are the manual worker.",
            "tools": ["read"],
            "max_steps": 2
        }))?,
    )?;

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
            "@allowed-worker",
            "Handle this directly.",
        ])
        .env_clear()
        .env("OPENAGENT_MOCK_SUBAGENT_ANSWER", "manual child answer")
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
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing manual task completion")?;
    assert_eq!(completed["params"]["manual"], true);
    assert_eq!(
        completed["params"]["metadata"]["subagent_type"],
        "allowed-worker"
    );
    assert!(
        completed["params"]["output"]
            .as_str()
            .is_some_and(|output| output.contains("manual child answer"))
    );
    let turn = events
        .iter()
        .find(|event| event["method"] == "turn/completed")
        .ok_or("missing completed turn")?;
    assert_eq!(turn["params"]["source"], "manual_subagent");
    assert_eq!(turn["params"]["tool_calls"], 1);

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
