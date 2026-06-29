#[test]
fn remote_runtime_client_uses_real_provider_endpoint_for_plain_turn() -> Result<(), Box<dyn Error>>
{
    let (provider_port, provider_thread) = spawn_fake_openai_responses_provider()?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-real-provider")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
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
    let started = client.start_turn(&session_id, "ask provider", serde_json::json!({}))?;

    assert_eq!(started["status"], "completed");
    assert_eq!(started["turn"]["final_answer"], "real provider answer");
    assert_eq!(started["turn"]["usage"]["input_tokens"], 7);
    assert_eq!(started["turn"]["usage"]["output_tokens"], 3);
    assert!(
        started["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|event| event["method"] == "item/agentMessage/delta"
                && event["params"]["delta"] == "real provider answer")
    );

    let _ = server.kill();
    let _ = provider_thread.join();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_continues_provider_after_tool_call() -> Result<(), Box<dyn Error>> {
    let first = serde_json::json!({
        "id": "resp_tool_call",
        "output": [{
            "type": "function_call",
            "call_id": "call_read_notes",
            "name": "read",
            "arguments": "{\"file_path\":\"notes.txt\"}"
        }],
        "usage": {"input_tokens": 5, "output_tokens": 1}
    });
    let second = serde_json::json!({
        "id": "resp_final",
        "output_text": "tool result says alpha",
        "usage": {"input_tokens": 9, "output_tokens": 4}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![first, second])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-provider-tool-loop")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    fs::write(workspace.join("notes.txt"), "alpha\n")?;
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
    let started = client.start_turn(&session_id, "read notes", serde_json::json!({}))?;

    assert_eq!(started["status"], "completed");
    assert_eq!(started["turn"]["final_answer"], "tool result says alpha");
    assert_eq!(started["turn"]["usage"]["input_tokens"], 14);
    assert_eq!(started["turn"]["usage"]["output_tokens"], 5);
    assert_eq!(started["turn"]["usage"]["tool_calls"], 1);
    let events = started["events"].as_array().expect("events");
    assert!(events.iter().any(|event| {
        event["method"] == "item/toolCall/completed"
            && event["params"]["call_id"] == "call_read_notes"
            && event["params"]["output"]
                .as_str()
                .is_some_and(|value| value.contains("alpha"))
    }));
    assert!(events.iter().any(|event| {
        event["method"] == "item/agentMessage/delta"
            && event["params"]["delta"] == "tool result says alpha"
    }));

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("function_call_output"));
    assert!(requests[1].contains("alpha"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_resumes_provider_after_question_reply() -> Result<(), Box<dyn Error>> {
    let first = serde_json::json!({
        "id": "resp_question",
        "output": [{
            "type": "function_call",
            "call_id": "call_question",
            "name": "question",
            "arguments": "{\"questions\":[{\"header\":\"Confirm\",\"question\":\"Proceed?\",\"multiple\":false,\"options\":[{\"label\":\"yes\",\"description\":\"Continue\"},{\"label\":\"no\",\"description\":\"Stop\"}]}]}"
        }],
        "usage": {"input_tokens": 4, "output_tokens": 1}
    });
    let second = serde_json::json!({
        "id": "resp_final",
        "output_text": "continuing after yes",
        "usage": {"input_tokens": 8, "output_tokens": 3}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![first, second])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-provider-question-resume")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
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
    let started = client.start_turn(&session_id, "ask a question", serde_json::json!({}))?;
    assert_eq!(started["status"], "waiting_question");
    let question = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["method"] == "item/question/requested")
        .and_then(|event| event["params"]["event"].as_object())
        .cloned()
        .expect("question event");
    let mut response = Value::Object(question);
    response["answers"] = serde_json::json!([["yes"]]);

    let resolved = client.respond_question(&response)?;
    assert_eq!(resolved["status"], "completed");
    assert_eq!(
        resolved["turn"]["final_answer"],
        serde_json::json!("continuing after yes")
    );
    let events = resolved["events"].as_array().expect("resolved events");
    assert!(events.iter().any(|event| {
        event["method"] == "item/toolCall/completed"
            && event["params"]["name"] == "question"
            && event["params"]["output"]
                .as_str()
                .is_some_and(|value| value.contains("yes"))
    }));
    assert!(events.iter().any(|event| {
        event["method"] == "turn/completed"
            && event["params"]["final_answer"] == "continuing after yes"
    }));
    let session = client.get_session(&session_id)?;
    assert!(session["metadata"]["pending_question"].is_null());
    assert!(session["metadata"]["pending_provider_turn"].is_null());

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("function_call_output"));
    assert!(requests[1].contains("yes"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_resumes_provider_after_approval_allow() -> Result<(), Box<dyn Error>> {
    let first = serde_json::json!({
        "id": "resp_approval",
        "output": [{
            "type": "function_call",
            "call_id": "call_bash",
            "name": "bash",
            "arguments": "{\"command\":\"printf approved\"}"
        }],
        "usage": {"input_tokens": 6, "output_tokens": 1}
    });
    let second = serde_json::json!({
        "id": "resp_final",
        "output_text": "approval flow completed",
        "usage": {"input_tokens": 10, "output_tokens": 4}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![first, second])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-provider-approval-resume")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
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
        "run command",
        serde_json::json!({"permission": "PLAN_ONLY"}),
    )?;
    assert_eq!(started["status"], "waiting_approval");
    let approval = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["method"] == "turn/approval_requested")
        .and_then(|event| event["params"]["approval"].as_object())
        .cloned()
        .expect("approval event");
    let mut response = Value::Object(approval);
    response["action"] = Value::String("allow".to_string());
    response["scope"] = Value::String("once".to_string());

    let resolved = client.respond_approval(&response)?;
    assert_eq!(resolved["status"], "completed");
    assert_eq!(
        resolved["turn"]["final_answer"],
        serde_json::json!("approval flow completed")
    );
    let events = resolved["events"].as_array().expect("resolved events");
    assert!(events.iter().any(|event| {
        event["method"] == "item/toolCall/completed"
            && event["params"]["name"] == "bash"
            && event["params"]["output"] == "approved"
    }));
    assert!(events.iter().any(|event| {
        event["method"] == "turn/completed"
            && event["params"]["final_answer"] == "approval flow completed"
    }));
    let session = client.get_session(&session_id)?;
    assert!(session["metadata"]["pending_approval"].is_null());
    assert!(session["metadata"]["pending_provider_turn"].is_null());

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("function_call_output"));
    assert!(requests[1].contains("approved"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}
