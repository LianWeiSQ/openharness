use super::*;

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
