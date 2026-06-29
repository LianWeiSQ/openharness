#[test]
fn live_sse_tails_interaction_resolved_events_before_provider_final() -> Result<(), Box<dyn Error>>
{
    run_live_interaction_resume_case("question")?;
    run_live_interaction_resume_case("approval")?;
    Ok(())
}

#[test]
fn global_sse_live_tails_provider_stream_delta_before_completion() -> Result<(), Box<dyn Error>> {
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_streaming_provider()?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-provider-live-stream")?;
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
    let live = thread::spawn(move || {
        http_request(
            port,
            "GET",
            "/api/events?last_event_id=0&live_timeout_ms=700",
            &[
                ("Authorization", "Bearer secret"),
                ("Accept", "text/event-stream"),
            ],
            "",
        )
        .map_err(|error| error.to_string())
    });
    thread::sleep(Duration::from_millis(150));

    let turn_session_id = session_id.clone();
    let turn = thread::spawn(move || {
        let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
            .with_auth(RemoteAuth::bearer("secret"));
        client
            .start_turn(
                &turn_session_id,
                "stream from provider",
                serde_json::json!({}),
            )
            .map_err(|error| error.to_string())
    });

    let live_response = live
        .join()
        .map_err(|_| "live sse thread panicked".to_string())?
        .map_err(|error| format!("live sse request failed: {error}"))?;
    assert!(live_response.contains("event: item/agentMessage/delta"));
    assert!(live_response.contains("streamed "));
    assert!(!live_response.contains("event: turn/completed"));

    let started = turn
        .join()
        .map_err(|_| "turn thread panicked".to_string())?
        .map_err(|error| format!("turn failed: {error}"))?;
    assert_eq!(started["status"], "completed");
    assert_eq!(started["turn"]["final_answer"], "streamed answer");
    assert_eq!(started["turn"]["usage"]["input_tokens"], 11);
    assert_eq!(started["turn"]["usage"]["output_tokens"], 2);

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("accept: text/event-stream")
    );
    assert!(requests[0].contains("\"stream\":true"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn global_sse_live_tails_provider_tool_events_before_final_answer() -> Result<(), Box<dyn Error>> {
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_streaming_tool_then_delayed_final_provider()?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-provider-tool-live-stream")?;
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
    let live = thread::spawn(move || {
        http_request(
            port,
            "GET",
            "/api/events?last_event_id=0&live_timeout_ms=800",
            &[
                ("Authorization", "Bearer secret"),
                ("Accept", "text/event-stream"),
            ],
            "",
        )
        .map_err(|error| error.to_string())
    });
    thread::sleep(Duration::from_millis(150));

    let turn_session_id = session_id.clone();
    let turn = thread::spawn(move || {
        let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
            .with_auth(RemoteAuth::bearer("secret"));
        client
            .start_turn(
                &turn_session_id,
                "read notes with live tool events",
                serde_json::json!({}),
            )
            .map_err(|error| error.to_string())
    });

    let live_response = live
        .join()
        .map_err(|_| "live sse thread panicked".to_string())?
        .map_err(|error| format!("live sse request failed: {error}"))?;
    assert!(live_response.contains("event: item/toolCall/started"));
    assert!(live_response.contains("event: item/toolCall/completed"));
    assert!(live_response.contains("call_live_read"));
    assert!(live_response.contains("alpha"));
    assert!(!live_response.contains("event: turn/completed"));

    let started = turn
        .join()
        .map_err(|_| "turn thread panicked".to_string())?
        .map_err(|error| format!("turn failed: {error}"))?;
    assert_eq!(started["status"], "completed");
    assert_eq!(started["turn"]["final_answer"], "tool final answer");

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
fn global_sse_live_tails_events_after_connection() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-live-sse")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let live = thread::spawn(move || {
        http_request(
            port,
            "GET",
            "/api/events?last_event_id=0&live_timeout_ms=5000",
            &[
                ("Authorization", "Bearer secret"),
                ("Accept", "text/event-stream"),
            ],
            "",
        )
        .map_err(|error| error.to_string())
    });
    thread::sleep(Duration::from_millis(150));

    let started = client.start_turn(
        &session_id,
        "write notes",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_live_write",
                "name": "write",
                "input": {"file_path": "live.txt", "content": "live\n"}
            }
        }),
    )?;
    assert_eq!(started["status"], "completed");

    let live_response = live
        .join()
        .map_err(|_| "live sse thread panicked".to_string())?
        .map_err(|error| format!("live sse request failed: {error}"))?;
    assert!(live_response.contains("content-type: text/event-stream"));
    assert!(
        !live_response
            .to_ascii_lowercase()
            .contains("content-length")
    );
    assert!(live_response.contains("event: item/toolCall/completed"));
    assert!(live_response.contains("event: turn/completed"));

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}
