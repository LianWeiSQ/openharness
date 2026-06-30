use super::*;

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
