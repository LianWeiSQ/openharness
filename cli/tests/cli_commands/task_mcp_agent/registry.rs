use super::*;

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
