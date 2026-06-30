#[test]
fn task_subagent_profile_max_steps_failure_propagates_to_parent() -> Result<(), Box<dyn Error>> {
    let child_tool_call = serde_json::json!({
        "id": "resp_child_tool_call",
        "output": [{
            "type": "function_call",
            "call_id": "call_read_notes",
            "name": "read",
            "arguments": "{\"file_path\":\"notes.txt\"}"
        }],
        "usage": {"input_tokens": 3, "output_tokens": 1}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![child_tool_call])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-task-subagent-max-steps")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    fs::write(workspace.join("notes.txt"), "alpha\n")?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("one-step.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "one-step",
            "name": "One Step",
            "description": "Subagent with a single provider step",
            "mode": "subagent",
            "permission": "READONLY",
            "prompt": "You are a constrained one-step reader.",
            "tools": ["read"],
            "max_steps": 1
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
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(
        &session_id,
        "delegate one-step",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task",
                "name": "task",
                "input": {
                    "description": "One step read",
                    "prompt": "Read notes and report back.",
                    "subagent_type": "one-step"
                }
            }
        }),
    )?;

    assert_eq!(started["status"], "completed");
    let events = started["events"].as_array().expect("events");
    let failed = events
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/failed" && event["params"]["name"] == "task"
        })
        .ok_or("missing failed task event")?;
    assert_eq!(failed["params"]["metadata"]["status"], "failed");
    assert_eq!(failed["params"]["metadata"]["max_steps"], 1);
    assert!(
        failed["params"]["error"]
            .as_str()
            .is_some_and(|value| value.contains("finished with status failed"))
    );
    let child_session_id = failed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?;
    let tasks = client.tasks(&session_id)?;
    let task = tasks
        .iter()
        .find(|task| task["session_id"] == child_session_id)
        .ok_or("missing failed subagent task lifecycle summary")?;
    assert_eq!(task["status"], "failed");
    assert_eq!(task["title"], "One step read");
    assert_eq!(task["subagent_type"], "one-step");
    assert_eq!(task["parent_tool_call_id"], "call_task");
    assert_eq!(task["max_steps"], 1);
    assert_eq!(task["run"]["status"], "failed");
    let child_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(child_session_id)
            .join("state.latest.json"),
    )?)?;
    assert_eq!(child_state["metadata"]["subagent"], true);
    assert_eq!(child_state["metadata"]["agent_profile"]["id"], "one-step");
    assert_eq!(child_state["metadata"]["max_steps"], 1);
    assert!(child_state["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("alpha"))
        })
    }));

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("Read notes and report back."));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn task_subagent_background_true_queues_queryable_task() -> Result<(), Box<dyn Error>> {
    let child_final = serde_json::json!({
        "id": "resp_child_background",
        "output_text": "background child answer",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![child_final])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-task-subagent-background")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("background-research.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "background-research",
            "name": "Background Research",
            "description": "Queued background research subagent",
            "mode": "subagent",
            "permission": "READONLY",
            "prompt": "You are a queued background researcher.",
            "tools": ["read"],
            "model": "background-child-model",
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
        "queue research",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task_background",
                "name": "task",
                "input": {
                    "description": "Queue background research",
                    "prompt": "Research this in the background.",
                    "subagent_type": "background-research",
                    "background": true
                }
            }
        }),
    )?;

    assert_eq!(started["status"], "completed");
    let events = started["events"].as_array().expect("events");
    let completed = events
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing queued task completion")?;
    assert_eq!(completed["params"]["metadata"]["status"], "queued");
    assert_eq!(completed["params"]["metadata"]["background"], true);
    assert!(
        completed["params"]["output"]
            .as_str()
            .is_some_and(|output| output.contains("state=\"queued\""))
    );
    let child_session_id = completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?;
    let tasks = client.tasks(&session_id)?;
    let task = tasks
        .iter()
        .find(|task| task["session_id"] == child_session_id)
        .ok_or("missing queued background task lifecycle summary")?;
    assert_eq!(task["status"], "queued");
    assert_eq!(task["background"], true);
    assert_eq!(task["title"], "Queue background research");
    assert_eq!(task["subagent_type"], "background-research");
    assert_eq!(task["parent_tool_call_id"], "call_task_background");
    assert_eq!(task["max_steps"], 2);
    assert_eq!(task["run_status"], Value::Null);
    let child_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(child_session_id)
            .join("state.latest.json"),
    )?)?;
    assert_eq!(child_state["status"], "idle");
    assert_eq!(child_state["metadata"]["task_status"], "queued");
    assert_eq!(child_state["metadata"]["background"], true);
    assert_eq!(
        child_state["metadata"]["agent_profile"]["id"],
        "background-research"
    );
    assert!(child_state["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("Research this in the background."))
        })
    }));

    assert_eq!(
        provider_requests.lock().expect("provider requests").len(),
        0
    );
    let ran = client.run_task(&session_id, child_session_id, serde_json::json!({}))?;
    assert_eq!(ran["status"], "completed");
    assert_eq!(ran["task"]["status"], "completed");
    assert_eq!(ran["task"]["run_status"], "completed");
    assert_eq!(ran["task"]["background"], true);
    assert_eq!(
        ran["result"]["turn"]["final_answer"],
        "background child answer"
    );
    let tasks = client.tasks(&session_id)?;
    let task = tasks
        .iter()
        .find(|task| task["session_id"] == child_session_id)
        .ok_or("missing completed background task lifecycle summary")?;
    assert_eq!(task["status"], "completed");
    assert_eq!(task["run_status"], "completed");
    let child_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(child_session_id)
            .join("state.latest.json"),
    )?)?;
    assert_eq!(child_state["metadata"]["task_status"], "completed");

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("Research this in the background."));
    assert!(requests[0].contains("background-child-model"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn task_subagent_background_worker_auto_runs_queued_task() -> Result<(), Box<dyn Error>> {
    let child_final = serde_json::json!({
        "id": "resp_child_background_worker",
        "output_text": "background worker answer",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![child_final])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-task-subagent-background-worker")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("worker-research.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "worker-research",
            "name": "Worker Research",
            "description": "Background worker research subagent",
            "mode": "subagent",
            "permission": "READONLY",
            "prompt": "You are an automatically scheduled background researcher.",
            "tools": ["read"],
            "model": "worker-child-model",
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
            ("OPENAGENT_BACKGROUND_WORKER_POLL_MS", "20"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(
        &session_id,
        "queue worker research",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task_background_worker",
                "name": "task",
                "input": {
                    "description": "Queue worker research",
                    "prompt": "Research this via the worker.",
                    "subagent_type": "worker-research",
                    "background": true
                }
            }
        }),
    )?;

    assert_eq!(started["status"], "completed");
    let completed = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing queued task completion")?;
    assert_eq!(completed["params"]["metadata"]["status"], "queued");
    let child_session_id = completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?;

    let task = wait_for_task_status(&client, &session_id, child_session_id, "completed")?;
    assert_eq!(task["run_status"], "completed");
    assert_eq!(task["background"], true);
    let child_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(child_session_id)
            .join("state.latest.json"),
    )?)?;
    assert_eq!(child_state["metadata"]["task_status"], "completed");
    assert_eq!(
        child_state["metadata"]["run_started_by"],
        "background_worker"
    );

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("Research this via the worker."));
    assert!(requests[0].contains("worker-child-model"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}
