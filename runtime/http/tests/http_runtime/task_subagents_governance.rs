#[test]
fn task_subagent_nested_tree_and_governance_guards() -> Result<(), Box<dyn Error>> {
    let outer_calls_inner = serde_json::json!({
        "id": "resp_outer_calls_inner",
        "output": [{
            "type": "function_call",
            "call_id": "call_nested_inner",
            "name": "task",
            "arguments": "{\"description\":\"Run nested inner\",\"prompt\":\"Inner should check nested guards.\",\"subagent_type\":\"inner\"}"
        }],
        "usage": {"input_tokens": 4, "output_tokens": 1}
    });
    let inner_calls_blocked_tasks = serde_json::json!({
        "id": "resp_inner_calls_blocked_tasks",
        "output": [
            {
                "type": "function_call",
                "call_id": "call_inner_self",
                "name": "task",
                "arguments": "{\"description\":\"Inner self recursion\",\"prompt\":\"This should be blocked by self-call guard.\",\"subagent_type\":\"inner\"}"
            },
            {
                "type": "function_call",
                "call_id": "call_too_deep",
                "name": "task",
                "arguments": "{\"description\":\"Too deep recursion\",\"prompt\":\"This should be blocked by depth guard.\",\"subagent_type\":\"third\"}"
            }
        ],
        "usage": {"input_tokens": 4, "output_tokens": 2}
    });
    let inner_final = serde_json::json!({
        "id": "resp_inner_final",
        "output_text": "inner handled guard failures",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let outer_final = serde_json::json!({
        "id": "resp_outer_final",
        "output_text": "outer nested answer",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![
            outer_calls_inner,
            inner_calls_blocked_tasks,
            inner_final,
            outer_final,
        ])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-task-subagent-nested")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    for id in ["outer", "inner", "third"] {
        fs::write(
            agent_dir.join(format!("{id}.json")),
            serde_json::to_string_pretty(&serde_json::json!({
                "id": id,
                "name": format!("{id} worker"),
                "description": format!("{id} nested subagent"),
                "mode": "subagent",
                "permission": "FULL",
                "prompt": format!("You are the {id} nested subagent."),
                "tools": ["task"],
                "model": "nested-child-model",
                "max_steps": 4
            }))?,
        )?;
    }
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
            ("OPENAGENT_MAX_SUBAGENT_DEPTH", "2"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(
        &session_id,
        "delegate nested work",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_task_outer",
                "name": "task",
                "input": {
                    "description": "Run outer nested work",
                    "prompt": "Outer should call inner.",
                    "subagent_type": "outer"
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
        .ok_or("missing outer task completion")?;
    assert_eq!(completed["params"]["metadata"]["subagent_type"], "outer");
    assert_eq!(completed["params"]["metadata"]["task_depth"], 1);
    assert!(completed["params"]["output"].as_str().is_some_and(
        |output| output.contains("<task id=") && output.contains("outer nested answer")
    ));
    let outer_session_id = completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing outer session id")?
        .to_string();

    let payload = json_body(&authorized_request(
        port,
        "GET",
        &format!("/api/sessions/{session_id}/tasks"),
        "",
        false,
    )?)?;
    assert_eq!(payload["tasks"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["tree"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["flat_tasks"].as_array().map(Vec::len), Some(2));
    let outer = payload["tree"][0].clone();
    assert_eq!(outer["session_id"], outer_session_id);
    assert_eq!(outer["subagent_type"], "outer");
    assert_eq!(outer["task_depth"], 1);
    assert_eq!(outer["task_root_session_id"], session_id);
    assert_eq!(
        outer["task_lineage_subagents"],
        serde_json::json!(["outer"])
    );
    let inner = outer["children"]
        .as_array()
        .and_then(|children| children.first())
        .ok_or("missing nested inner task")?;
    let inner_session_id = inner["session_id"]
        .as_str()
        .ok_or("missing inner session id")?;
    assert_eq!(inner["subagent_type"], "inner");
    assert_eq!(inner["parent_session_id"], outer_session_id);
    assert_eq!(inner["task_parent_session_id"], outer_session_id);
    assert_eq!(inner["task_root_session_id"], session_id);
    assert_eq!(inner["task_depth"], 2);
    assert_eq!(
        inner["task_lineage_subagents"],
        serde_json::json!(["outer", "inner"])
    );
    assert_eq!(inner["children"].as_array().map(Vec::len), Some(0));

    let inner_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(inner_session_id)
            .join("state.latest.json"),
    )?)?;
    assert_eq!(inner_state["metadata"]["task_depth"], 2);
    assert_eq!(
        inner_state["metadata"]["task_lineage_subagents"],
        serde_json::json!(["outer", "inner"])
    );
    assert!(
        !fs::read_dir(&session_root)?.flatten().any(|entry| {
            let state_path = entry.path().join("state.latest.json");
            let Ok(raw) = fs::read_to_string(state_path) else {
                return false;
            };
            let Ok(state) = serde_json::from_str::<Value>(&raw) else {
                return false;
            };
            state["metadata"]["task_description"] == "Inner self recursion"
                || state["metadata"]["task_description"] == "Too deep recursion"
        }),
        "blocked nested task calls must not create child sessions"
    );

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 4);
    assert!(requests[0].contains("Outer should call inner."));
    assert!(requests[1].contains("Inner should check nested guards."));
    assert!(requests[1].contains("Available subagents: none."));
    assert!(requests[2].contains("subagent inner cannot call itself"));
    assert!(requests[2].contains("exceeds max subagent depth 2"));
    assert!(requests[3].contains("inner handled guard failures"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn task_subagent_loads_opencode_markdown_agent_options() -> Result<(), Box<dyn Error>> {
    let child_final = serde_json::json!({
        "id": "resp_markdown_agent",
        "output_text": "markdown agent answer",
        "usage": {"input_tokens": 4, "output_tokens": 2}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![child_final])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-opencode-agent-md")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".opencode/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("markdown-research.md"),
        r#"---
id: markdown-research
name: Markdown Research
description: OpenCode markdown research agent
mode: subagent
permission: READONLY
tools: ["read"]
model: markdown-child-model
steps: 2
temperature: 0.21
top_p: 0.82
reasoning_effort: medium
color: cyan
---
You are the Markdown research subagent.
"#,
    )?;
    fs::write(
        agent_dir.join("hidden-worker.md"),
        r#"---
id: hidden-worker
name: Hidden Worker
description: Hidden markdown agent
mode: subagent
hidden: true
---
Hidden prompt.
"#,
    )?;
    fs::write(
        agent_dir.join("disabled-worker.md"),
        r#"---
id: disabled-worker
name: Disabled Worker
description: Disabled markdown agent
mode: subagent
disable: true
---
Disabled prompt.
"#,
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
    let agents = client.agents()?;
    let agent_items = agents["agents"].as_array().ok_or("missing agents")?;
    let markdown_agent = agent_items
        .iter()
        .find(|agent| agent["id"] == "markdown-research")
        .ok_or("missing markdown agent")?;
    assert_eq!(markdown_agent["name"], "Markdown Research");
    assert_eq!(
        markdown_agent["description"],
        "OpenCode markdown research agent"
    );
    assert_eq!(markdown_agent["steps"], 2);
    assert_eq!(markdown_agent["max_steps"], 2);
    assert_eq!(markdown_agent["temperature"], 0.21);
    assert_eq!(markdown_agent["top_p"], 0.82);
    assert_eq!(markdown_agent["color"], "cyan");
    assert_eq!(
        markdown_agent["model_options"]["reasoning_effort"],
        "medium"
    );
    assert!(
        !agent_items
            .iter()
            .any(|agent| agent["id"] == "hidden-worker")
    );
    assert!(
        !agent_items
            .iter()
            .any(|agent| agent["id"] == "disabled-worker")
    );

    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(
        &session_id,
        "delegate markdown agent",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_markdown_agent",
                "name": "task",
                "input": {
                    "description": "Run markdown agent",
                    "prompt": "Use the markdown agent prompt.",
                    "subagent_type": "markdown-research"
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
        .ok_or("missing markdown task completion")?;
    assert_eq!(
        completed["params"]["metadata"]["subagent_type"],
        "markdown-research"
    );
    assert_eq!(completed["params"]["metadata"]["max_steps"], 2);
    assert_eq!(
        completed["params"]["metadata"]["model_options"]["reasoning_effort"],
        "medium"
    );
    let child_session_id = completed["params"]["metadata"]["session_id"]
        .as_str()
        .ok_or("missing child session id")?;
    let child_state: Value = serde_json::from_str(&fs::read_to_string(
        session_root
            .join(child_session_id)
            .join("state.latest.json"),
    )?)?;
    assert_eq!(
        child_state["messages"][0]["content"],
        "You are the Markdown research subagent."
    );
    assert_eq!(child_state["metadata"]["temperature"], 0.21);
    assert_eq!(child_state["metadata"]["top_p"], 0.82);
    assert_eq!(child_state["metadata"]["color"], "cyan");

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 1);
    let provider_request: Value = serde_json::from_str(&requests[0])?;
    assert_eq!(provider_request["model"], "markdown-child-model");
    assert_eq!(provider_request["temperature"], 0.21);
    assert_eq!(provider_request["top_p"], 0.82);
    assert_eq!(provider_request["reasoning_effort"], "medium");
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn task_tool_respects_agent_task_permissions() -> Result<(), Box<dyn Error>> {
    let forged_task = serde_json::json!({
        "id": "resp_parent_forged_task",
        "output": [{
            "type": "function_call",
            "call_id": "call_task_blocked",
            "name": "task",
            "arguments": "{\"description\":\"Blocked task\",\"prompt\":\"Should not run.\",\"subagent_type\":\"blocked-worker\"}"
        }],
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let parent_final = serde_json::json!({
        "id": "resp_parent_final_after_denial",
        "output_text": "parent saw task denial",
        "usage": {"input_tokens": 6, "output_tokens": 3}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![forged_task, parent_final])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-task-permissions")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("limited-primary.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "limited-primary",
            "name": "Limited Primary",
            "description": "Primary agent that may only launch allowed-worker.",
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
            serde_json::to_string_pretty(&serde_json::json!({
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
        "try a blocked subagent",
        serde_json::json!({
            "permission": "FULL",
            "agent": "limited-primary",
            "max_steps": 2
        }),
    )?;

    assert_eq!(started["status"], "completed");
    assert_eq!(started["turn"]["final_answer"], "parent saw task denial");
    let events = started["events"].as_array().expect("events");
    let failed = events
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/failed" && event["params"]["name"] == "task"
        })
        .ok_or("missing denied task event")?;
    assert_eq!(failed["params"]["metadata"]["permission_action"], "deny");
    assert_eq!(
        failed["params"]["metadata"]["permission_pattern"],
        "blocked-worker"
    );
    let tasks = client.tasks(&session_id)?;
    assert!(
        tasks.is_empty(),
        "denied task should not create a child task"
    );

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    let first_request: Value = serde_json::from_str(&requests[0])?;
    let task_tool = first_request["tools"]
        .as_array()
        .ok_or("missing tools")?
        .iter()
        .find(|tool| tool["name"] == "task" || tool["function"]["name"] == "task")
        .ok_or("missing task tool")?;
    let task_description = task_tool
        .get("description")
        .or_else(|| task_tool.pointer("/function/description"))
        .and_then(Value::as_str)
        .ok_or("missing task tool description")?;
    assert!(task_description.contains("allowed-worker"));
    assert!(!task_description.contains("blocked-worker"));
    assert!(requests[1].contains("Permission denied"));
    assert!(requests[1].contains("blocked-worker"));

    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn start_turn_invokes_subagent_with_at_mention() -> Result<(), Box<dyn Error>> {
    let child_final = serde_json::json!({
        "id": "resp_manual_child",
        "output_text": "manual http child answer",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![child_final])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-at-subagent")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let agent_dir = workspace.join(".openagent/agents");
    fs::create_dir_all(&agent_dir)?;
    fs::write(
        agent_dir.join("limited-primary.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "limited-primary",
            "name": "Limited Primary",
            "description": "Primary agent that may only launch allowed-worker.",
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
            serde_json::to_string_pretty(&serde_json::json!({
                "id": id,
                "name": id,
                "description": format!("{id} subagent"),
                "mode": "subagent",
                "permission": "READONLY",
                "prompt": format!("You are {id}."),
                "tools": ["read"],
                "model": "manual-child-model",
                "max_steps": 2
            }))?,
        )?;
    }
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
    let allowed_session_id = client.create_session(&workspace, None)?;
    let allowed = client.start_turn(
        &allowed_session_id,
        "@allowed-worker Handle this directly.",
        serde_json::json!({
            "permission": "FULL",
            "agent": "limited-primary"
        }),
    )?;
    assert_eq!(allowed["status"], "completed");
    let allowed_events = allowed["events"].as_array().expect("events");
    let completed = allowed_events
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["name"] == "task"
        })
        .ok_or("missing manual task completion")?;
    assert_eq!(
        completed["params"]["metadata"]["subagent_type"],
        "allowed-worker"
    );
    assert!(
        completed["params"]["output"]
            .as_str()
            .is_some_and(|output| output.contains("manual http child answer"))
    );
    let allowed_tasks = client.tasks(&allowed_session_id)?;
    assert_eq!(allowed_tasks.len(), 1);
    assert_eq!(allowed_tasks[0]["subagent_type"], "allowed-worker");

    let denied_session_id = client.create_session(&workspace, None)?;
    let denied = client.start_turn(
        &denied_session_id,
        "@blocked-worker Should not run.",
        serde_json::json!({
            "permission": "FULL",
            "agent": "limited-primary"
        }),
    )?;
    assert_eq!(denied["status"], "completed");
    let denied_events = denied["events"].as_array().expect("events");
    let failed = denied_events
        .iter()
        .find(|event| {
            event["method"] == "item/toolCall/failed" && event["params"]["name"] == "task"
        })
        .ok_or("missing denied manual task failure")?;
    assert_eq!(failed["params"]["metadata"]["permission_action"], "deny");
    assert_eq!(
        failed["params"]["metadata"]["permission_pattern"],
        "blocked-worker"
    );
    assert!(client.tasks(&denied_session_id)?.is_empty());

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("Handle this directly."));
    assert!(requests[0].contains("manual-child-model"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}
