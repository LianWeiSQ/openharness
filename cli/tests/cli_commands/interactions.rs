#[test]
fn binary_run_queues_approval_for_dangerous_tool() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-agent-approval")?;
    let session_root = temp.join("sessions");
    let output = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "run",
            "a",
            "command",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_bash","name":"bash","input":{"command":"echo hi"}}]"#,
        )
        .output()?;
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    let events = stdout
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "turn/approval_requested")
    );
    let approval = events
        .iter()
        .find(|event| event["method"] == "turn/approval_requested")
        .expect("approval event");
    assert_eq!(approval["params"]["approval"]["tool_name"], "bash");
    assert_eq!(
        approval["params"]["approval"]["reason"],
        "permission_required"
    );
    let completed = events
        .iter()
        .find(|event| event["method"] == "turn/completed")
        .expect("failed completion event");
    assert_eq!(completed["params"]["status"], "paused");

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_approval_and_question_responses_resume_paused_runs() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-resume-queues")?;
    let session_root = temp.join("sessions");

    let approval_pause = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "run",
            "approval",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_bash","name":"bash","input":{"command":"printf approved"}}]"#,
        )
        .output()?;
    assert!(!approval_pause.status.success());
    let approval_events = String::from_utf8(approval_pause.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let approval = approval_events
        .iter()
        .find(|event| event["method"] == "turn/approval_requested")
        .ok_or("missing approval request")?;
    let approval_session = approval["params"]["session_id"]
        .as_str()
        .unwrap_or_default();
    let approval_response = run_openagent(
        [
            "approval",
            "respond",
            "--session-root",
            path_str(&session_root),
            "--session",
            approval_session,
            "--decision",
            "allow_once",
        ],
        None,
    )?;
    assert!(
        approval_response.status.success(),
        "{}",
        String::from_utf8_lossy(&approval_response.stderr)
    );
    let approval_resume = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--continue",
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
        ])
        .env_clear()
        .env("OPENAGENT_MOCK_ANSWER", "approval complete")
        .output()?;
    assert!(
        approval_resume.status.success(),
        "{}",
        String::from_utf8_lossy(&approval_resume.stderr)
    );
    let approval_resume_events = String::from_utf8(approval_resume.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(approval_resume_events.iter().any(|event| {
        event["method"] == "item/toolCall/completed" && event["params"]["output"] == "approved"
    }));
    assert!(approval_resume_events.iter().any(|event| {
        event["method"] == "turn/completed"
            && event["params"]["final_answer"] == "approval complete"
    }));

    let question_pause = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "ask",
            "question",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_question","name":"question","input":{"questions":[{"question":"Pick a mode","header":"Mode","options":[{"label":"Fast","description":"Use fast path"}]}]}}]"#,
        )
        .output()?;
    assert!(!question_pause.status.success());
    let question_events = String::from_utf8(question_pause.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let question = question_events
        .iter()
        .find(|event| event["method"] == "turn/question_requested")
        .ok_or("missing question request")?;
    let question_session = question["params"]["session_id"]
        .as_str()
        .unwrap_or_default();
    let question_response = run_openagent(
        [
            "question",
            "reply",
            "--session-root",
            path_str(&session_root),
            "--session",
            question_session,
            "--answer",
            "Fast",
        ],
        None,
    )?;
    assert!(
        question_response.status.success(),
        "{}",
        String::from_utf8_lossy(&question_response.stderr)
    );
    let question_resume = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--continue",
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
        ])
        .env_clear()
        .env("OPENAGENT_MOCK_ANSWER", "question complete")
        .output()?;
    assert!(
        question_resume.status.success(),
        "{}",
        String::from_utf8_lossy(&question_resume.stderr)
    );
    let question_resume_events = String::from_utf8(question_resume.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(question_resume_events.iter().any(|event| {
        event["method"] == "item/toolCall/completed"
            && event["params"]["output"]
                .as_str()
                .is_some_and(|text| text.contains("\"Pick a mode\"=\"Fast\""))
    }));
    assert!(question_resume_events.iter().any(|event| {
        event["method"] == "turn/completed"
            && event["params"]["final_answer"] == "question complete"
    }));

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn binary_run_skip_permissions_auto_allows_ask_but_not_deny() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-permission-skip")?;
    let session_root = temp.join("sessions");
    let allowed = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--dangerously-skip-permissions",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "run",
            "a",
            "command",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_bash","name":"bash","input":{"command":"printf allowed"}}]"#,
        )
        .env("OPENAGENT_MOCK_ANSWER", "done")
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
    assert!(
        allowed_events
            .iter()
            .any(|event| event["method"] == "item/toolCall/completed"
                && event["params"]["output"] == "allowed")
    );

    let denied_path = temp.join("denied.txt");
    let denied = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--permission",
            "READONLY",
            "--dangerously-skip-permissions",
            "--workspace",
            path_str(&temp),
            "--session-root",
            path_str(&session_root),
            "--format",
            "json",
            "write",
            "a",
            "file",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_write","name":"write","input":{"file_path":"denied.txt","content":"nope"}}]"#,
        )
        .env("OPENAGENT_MOCK_ANSWER", "blocked")
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
        .expect("denied tool failure event");
    assert_eq!(failed["params"]["metadata"]["permission_action"], "deny");
    assert_eq!(
        failed["params"]["metadata"]["error_kind"],
        "permission_denied"
    );
    assert!(!denied_path.exists());

    let _ = fs::remove_dir_all(temp);
    Ok(())
}
