#[test]
fn task_subagent_run_rejects_duplicate_consumer() -> Result<(), Box<dyn Error>> {
    let child_final = serde_json::json!({
        "id": "resp_child_duplicate",
        "output_text": "duplicate guarded answer",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence_with_delays(vec![(child_final, 900)])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-task-subagent-duplicate-run")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("single-consumer.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "single-consumer",
            "name": "Single Consumer",
            "description": "Queued subagent used for duplicate run tests",
            "mode": "subagent",
            "permission": "READONLY",
            "prompt": "You are a single-consumer background subagent.",
            "tools": ["read"],
            "model": "single-consumer-model",
            "max_steps": 2
        }))?,
    )?;
    let provider_base_url = format!("http://127.0.0.1:{provider_port}/v1");
    let mut server = spawn_runtime_with_env(
        port,
        &workspace,
        &session_root,
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", provider_base_url.as_str()),
            ("OPENAI_WIRE_API", "responses"),
            ("OPENAI_MODEL", "fake-model"),
            ("OPENAGENT_BACKGROUND_WORKER", "0"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(
        &session_id,
        "queue duplicate guarded task",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task_duplicate",
                "name": "task",
                "input": {
                    "description": "Duplicate guarded task",
                    "prompt": "Run once only.",
                    "subagent_type": "single-consumer",
                    "background": true
                }
            }
        }),
    )?;
    let completed = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing queued task completion")?;
    let child_session_id = completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?
        .to_string();

    let first_client = client.clone();
    let first_session_id = session_id.clone();
    let first_task_id = child_session_id.clone();
    let first = thread::spawn(move || {
        first_client.run_task(&first_session_id, &first_task_id, serde_json::json!({}))
    });
    thread::sleep(Duration::from_millis(150));
    let duplicate_error = client
        .run_task(&session_id, &child_session_id, serde_json::json!({}))
        .expect_err("duplicate task run should fail");
    assert!(
        duplicate_error.contains("task is already running")
            || duplicate_error.contains("task is not queued: running")
    );
    let first_result = first
        .join()
        .map_err(|_| "first task run thread panicked".to_string())?
        .map_err(|error| format!("first task run failed: {error}"))?;
    assert_eq!(first_result["status"], "completed");
    assert_eq!(
        first_result["result"]["turn"]["final_answer"],
        "duplicate guarded answer"
    );
    let tasks = client.tasks(&session_id)?;
    let task = tasks
        .iter()
        .find(|task| task["session_id"] == child_session_id)
        .ok_or("missing completed duplicate-guarded task")?;
    assert_eq!(task["status"], "completed");
    assert_eq!(task["run_status"], "completed");

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("Run once only."));
    assert!(requests[0].contains("single-consumer-model"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn task_subagent_run_recovers_stale_lock() -> Result<(), Box<dyn Error>> {
    let child_final = serde_json::json!({
        "id": "resp_child_stale_lock",
        "output_text": "stale lock recovered answer",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![child_final])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-task-subagent-stale-lock")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("stale-lock-runner.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "stale-lock-runner",
            "name": "Stale Lock Runner",
            "description": "Queued subagent used for stale lock recovery tests",
            "mode": "subagent",
            "permission": "READONLY",
            "prompt": "You are a stale-lock recovery background subagent.",
            "tools": ["read"],
            "model": "stale-lock-model",
            "max_steps": 2
        }))?,
    )?;
    let provider_base_url = format!("http://127.0.0.1:{provider_port}/v1");
    let mut server = spawn_runtime_with_env(
        port,
        &workspace,
        &session_root,
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", provider_base_url.as_str()),
            ("OPENAI_WIRE_API", "responses"),
            ("OPENAI_MODEL", "fake-model"),
            ("OPENAGENT_BACKGROUND_WORKER", "0"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(
        &session_id,
        "queue stale lock recoverable task",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task_stale_lock",
                "name": "task",
                "input": {
                    "description": "Stale lock recoverable task",
                    "prompt": "Recover from an abandoned lock.",
                    "subagent_type": "stale-lock-runner",
                    "background": true
                }
            }
        }),
    )?;
    let completed = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing queued task completion")?;
    let child_session_id = completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?
        .to_string();
    let stale_lock_path = session_root.join(&child_session_id).join("task.run.lock");
    fs::write(
        &stale_lock_path,
        serde_json::to_string(&serde_json::json!({
            "task_id": child_session_id,
            "claimed_at_ms": 0
        }))?,
    )?;

    let ran = client.run_task(&session_id, &child_session_id, serde_json::json!({}))?;
    assert_eq!(ran["status"], "completed");
    assert_eq!(ran["task"]["status"], "completed");
    assert_eq!(
        ran["result"]["turn"]["final_answer"],
        "stale lock recovered answer"
    );
    assert!(!stale_lock_path.exists());
    let tasks = client.tasks(&session_id)?;
    let task = tasks
        .iter()
        .find(|task| task["session_id"] == child_session_id)
        .ok_or("missing recovered stale-lock task")?;
    assert_eq!(task["status"], "completed");
    assert_eq!(task["run_status"], "completed");

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("Recover from an abandoned lock."));
    assert!(requests[0].contains("stale-lock-model"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn task_subagent_cancel_rejects_later_run() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-task-subagent-cancel")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("cancel-me.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "cancel-me",
            "name": "Cancel Me",
            "description": "Queued subagent used for cancel tests",
            "mode": "subagent",
            "permission": "READONLY",
            "prompt": "You are a queued subagent that should be canceled.",
            "tools": ["read"],
            "max_steps": 2
        }))?,
    )?;
    let mut server = spawn_runtime_with_env(
        port,
        &workspace,
        &session_root,
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAGENT_BACKGROUND_WORKER", "0"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(
        &session_id,
        "queue cancelable task",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task_cancel",
                "name": "task",
                "input": {
                    "description": "Cancelable task",
                    "prompt": "Do not actually run.",
                    "subagent_type": "cancel-me",
                    "background": true
                }
            }
        }),
    )?;
    let completed = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing queued task completion")?;
    let child_session_id = completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?;
    let stale_lock_path = session_root.join(child_session_id).join("task.run.lock");
    fs::write(
        &stale_lock_path,
        serde_json::to_string(&serde_json::json!({
            "task_id": child_session_id,
            "claimed_at_ms": 0
        }))?,
    )?;

    let canceled = client.cancel_task(&session_id, child_session_id)?;
    assert_eq!(canceled["status"], "canceled");
    assert_eq!(canceled["task"]["status"], "canceled");
    assert_eq!(canceled["task"]["background"], true);
    assert!(!stale_lock_path.exists());
    let tasks = client.tasks(&session_id)?;
    let task = tasks
        .iter()
        .find(|task| task["session_id"] == child_session_id)
        .ok_or("missing canceled task lifecycle summary")?;
    assert_eq!(task["status"], "canceled");
    assert_eq!(task["title"], "Cancelable task");
    let run_error = client
        .run_task(&session_id, child_session_id, serde_json::json!({}))
        .expect_err("canceled task run should fail");
    assert!(run_error.contains("task is not queued: canceled"));
    let child_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(child_session_id)
            .join("state.latest.json"),
    )?)?;
    assert_eq!(child_state["metadata"]["task_status"], "canceled");
    assert!(child_state["metadata"]["canceled_at_ms"].as_u64().is_some());

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}
