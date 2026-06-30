#[test]
fn remote_runtime_client_executes_task_subagent_tool() -> Result<(), Box<dyn Error>> {
    let child_forbidden_tool = serde_json::json!({
        "id": "resp_child_tool_call",
        "output": [{
            "type": "function_call",
            "call_id": "call_write_forbidden",
            "name": "write",
            "arguments": "{\"file_path\":\"blocked.txt\",\"content\":\"nope\"}"
        }],
        "usage": {"input_tokens": 3, "output_tokens": 1}
    });
    let child_final = serde_json::json!({
        "id": "resp_child",
        "output_text": "runtime child answer",
        "usage": {"input_tokens": 4, "output_tokens": 2}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![child_forbidden_tool, child_final])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-task-subagent")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("deep-research.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "deep-research",
            "name": "Deep Research",
            "description": "Workspace-defined research subagent",
            "mode": "subagent",
            "permission": "READONLY",
            "prompt": "You are the Custom runtime researcher.",
            "tools": ["read"],
            "model": "custom-child-model",
            "max_steps": 3
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
    let agents = client.agents()?;
    assert!(agents["agents"].as_array().is_some_and(|items| {
        items.iter().any(|agent| {
            agent["id"] == "deep-research"
                && agent["description"] == "Workspace-defined research subagent"
                && agent["model"] == "custom-child-model"
                && agent["tools"] == serde_json::json!(["read"])
        })
    }));
    let started = client.start_turn(
        &session_id,
        "delegate exploration",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task",
                "name": "task",
                "input": {
                    "description": "Explore runtime fixture",
                    "prompt": "Summarize this runtime fixture.",
                    "subagent_type": "deep-research"
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
        .ok_or("missing task completion")?;
    assert_eq!(
        completed["params"]["metadata"]["subagent_type"],
        "deep-research"
    );
    assert!(completed["params"]["output"].as_str().is_some_and(
        |output| output.contains("<task id=") && output.contains("runtime child answer")
    ));
    let child_session_id = completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?;
    let child_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(child_session_id)
            .join("state.latest.json"),
    )?)?;
    assert_eq!(child_state["metadata"]["subagent"], true);
    assert_eq!(child_state["metadata"]["parent_session_id"], session_id);
    assert_eq!(child_state["metadata"]["parent_tool_call_id"], "call_task");
    assert_eq!(
        child_state["metadata"]["agent_profile"]["id"],
        "deep-research"
    );
    assert_eq!(child_state["metadata"]["model"], "custom-child-model");
    assert_eq!(child_state["metadata"]["permission"], "READONLY");
    assert_eq!(
        child_state["metadata"]["agent_profile"]["tools"],
        serde_json::json!(["read"])
    );
    let tasks = client.tasks(&session_id)?;
    let task = tasks
        .iter()
        .find(|task| task["session_id"] == child_session_id)
        .ok_or("missing subagent task lifecycle summary")?;
    assert_eq!(task["status"], "completed");
    assert_eq!(task["title"], "Explore runtime fixture");
    assert_eq!(task["subagent_type"], "deep-research");
    assert_eq!(task["parent_tool_call_id"], "call_task");
    assert_eq!(task["agent_profile"]["id"], "deep-research");
    assert_eq!(task["run"]["status"], "completed");
    assert!(child_state["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["role"] == "system"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("Custom runtime researcher"))
        })
    }));

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    let first_request: Value = serde_json::from_str(&requests[0])?;
    let tool_names = first_request["tools"]
        .as_array()
        .ok_or("missing tools")?
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(tool_names, vec!["read"]);
    assert!(requests[0].contains("Summarize this runtime fixture."));
    assert!(requests[0].contains("custom-child-model"));
    assert!(requests[1].contains("function_call_output"));
    assert!(requests[1].contains("not available to this agent profile"));
    assert!(!workspace.join("blocked.txt").exists());
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn task_subagent_task_id_resumes_existing_child_session() -> Result<(), Box<dyn Error>> {
    let child_first = serde_json::json!({
        "id": "resp_child_resume_first",
        "output_text": "first child answer",
        "usage": {"input_tokens": 4, "output_tokens": 2}
    });
    let child_second = serde_json::json!({
        "id": "resp_child_resume_second",
        "output_text": "second child answer",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![child_first, child_second])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-task-subagent-resume")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("resume-worker.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "resume-worker",
            "name": "Resume Worker",
            "description": "Subagent used to verify task_id resume",
            "mode": "subagent",
            "permission": "READONLY",
            "prompt": "You are a resumable runtime subagent.",
            "tools": ["read"],
            "model": "resume-child-model",
            "max_steps": 2
        }))?,
    )?;
    fs::write(
        agent_dir.join("other-worker.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "other-worker",
            "name": "Other Worker",
            "description": "Subagent used to reject mismatched task resumes",
            "mode": "subagent",
            "permission": "READONLY",
            "prompt": "You are not the resumable worker.",
            "tools": ["read"],
            "model": "other-child-model",
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
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let first = client.start_turn(
        &session_id,
        "delegate resumable work",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task_first",
                "name": "task",
                "input": {
                    "description": "Initial resumable work",
                    "prompt": "First prompt for the resumable worker.",
                    "subagent_type": "resume-worker"
                }
            }
        }),
    )?;
    let first_completed = first["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing first task completion")?;
    let child_session_id = first_completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?
        .to_string();

    let wrong_agent = client.start_turn(
        &session_id,
        "try wrong subagent resume",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task_wrong_agent",
                "name": "task",
                "input": {
                    "description": "Wrong agent resume",
                    "prompt": "This should not run.",
                    "subagent_type": "other-worker",
                    "task_id": child_session_id.clone()
                }
            }
        }),
    )?;
    let wrong_agent_failed = wrong_agent["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/failed" && event["params"]["name"] == "task"
        })
        .ok_or("missing wrong-agent task failure")?;
    assert!(
        wrong_agent_failed["params"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("belongs to subagent resume-worker"))
    );

    let other_session_id = client.create_session(&workspace, None)?;
    let wrong_parent = client.start_turn(
        &other_session_id,
        "try wrong parent resume",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task_wrong_parent",
                "name": "task",
                "input": {
                    "description": "Wrong parent resume",
                    "prompt": "This should not run either.",
                    "subagent_type": "resume-worker",
                    "task_id": child_session_id.clone()
                }
            }
        }),
    )?;
    let wrong_parent_failed = wrong_parent["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/failed" && event["params"]["name"] == "task"
        })
        .ok_or("missing wrong-parent task failure")?;
    assert!(
        wrong_parent_failed["params"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("task does not belong to parent session"))
    );

    let resumed = client.start_turn(
        &session_id,
        "continue resumable work",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task_resume",
                "name": "task",
                "input": {
                    "description": "Continue resumable work",
                    "prompt": "Second prompt for the same resumable worker.",
                    "subagent_type": "resume-worker",
                    "task_id": child_session_id.clone()
                }
            }
        }),
    )?;
    let resumed_completed = resumed["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing resumed task completion")?;
    assert_eq!(
        resumed_completed["params"]["metadata"]["session_id"],
        child_session_id
    );
    assert_eq!(
        resumed_completed["params"]["metadata"]["status"],
        "completed"
    );
    assert!(
        resumed_completed["params"]["output"]
            .as_str()
            .is_some_and(
                |output| output.contains("<task id=") && output.contains("second child answer")
            )
    );

    let child_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(&child_session_id)
            .join("state.latest.json"),
    )?)?;
    assert_eq!(child_state["metadata"]["parent_session_id"], session_id);
    assert_eq!(
        child_state["metadata"]["parent_tool_call_id"],
        "call_task_resume"
    );
    assert_eq!(child_state["metadata"]["task_resume_count"], 1);
    assert!(
        child_state["metadata"]["task_resumed_at_ms"]
            .as_u64()
            .is_some_and(|value| value > 0)
    );
    let messages = child_state["messages"]
        .as_array()
        .ok_or("missing child messages")?;
    let system_count = messages
        .iter()
        .filter(|message| {
            message["role"] == "system" && message["metadata"]["agent_profile"] == "resume-worker"
        })
        .count();
    assert_eq!(system_count, 1);
    assert!(messages.iter().any(|message| {
        message["role"] == "user" && message["content"] == "First prompt for the resumable worker."
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"] == "Second prompt for the same resumable worker."
    }));

    let tasks = client.tasks(&session_id)?;
    let task = tasks
        .iter()
        .find(|task| task["session_id"] == child_session_id)
        .ok_or("missing resumed task summary")?;
    assert_eq!(task["status"], "completed");
    assert_eq!(task["title"], "Continue resumable work");
    assert_eq!(task["subagent_type"], "resume-worker");

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("First prompt for the resumable worker."));
    assert!(requests[1].contains("First prompt for the resumable worker."));
    assert!(requests[1].contains("Second prompt for the same resumable worker."));
    assert!(requests[1].contains("resume-child-model"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}
