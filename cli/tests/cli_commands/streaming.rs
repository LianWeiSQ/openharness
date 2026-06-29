#[test]
fn binary_run_streams_openai_chat_sse_provider_events() -> Result<(), Box<dyn Error>> {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"streamed\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n"
    )
    .to_string();
    let (port, server) = serve_http_once_on_free_port("text/event-stream", body)?;
    let output = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--provider",
            "openai",
            "--api-key",
            "secret",
            "--base-url",
            &format!("http://127.0.0.1:{port}"),
            "--wire-api",
            "chat",
            "--stream",
            "--format",
            "json",
            "hello",
        ])
        .env_clear()
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server
        .join()
        .expect("provider server thread")
        .expect("provider response");
    let events = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(events.iter().any(|event| {
        event["method"] == "item/agentMessage/delta"
            && event["params"]["delta"]
                .as_str()
                .is_some_and(|text| text.contains("hello ") || text.contains("streamed"))
    }));
    assert!(events.iter().any(|event| {
        event["method"] == "turn/completed" && event["params"]["source"] == "openai:chat:stream"
    }));
    Ok(())
}

#[test]
fn binary_run_emits_provider_sse_delta_before_stream_closes() -> Result<(), Box<dyn Error>> {
    let (port, server, release_server, server_timed_out) = serve_dripping_sse_provider()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "run",
            "--skip-doctor",
            "--provider",
            "openai",
            "--api-key",
            "secret",
            "--base-url",
            &format!("http://127.0.0.1:{port}"),
            "--wire-api",
            "chat",
            "--stream",
            "--format",
            "json",
            "hello",
        ])
        .env_clear()
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let mut reader = BufReader::new(stdout);
    let mut first_line = String::new();
    reader.read_line(&mut first_line)?;
    assert!(
        !server_timed_out.load(Ordering::SeqCst),
        "first stream event should arrive before the mock server closes"
    );
    let _ = release_server.send(());
    let first_event: Value = serde_json::from_str(first_line.trim())?;
    assert_eq!(first_event["method"], "item/agentMessage/delta");
    assert_eq!(first_event["params"]["delta"], "hello ");

    let mut rest = String::new();
    reader.read_to_string(&mut rest)?;
    let status = child.wait()?;
    assert!(status.success());
    server
        .join()
        .expect("provider server thread")
        .expect("provider response");
    let events = rest
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(events.iter().any(|event| {
        event["method"] == "item/agentMessage/delta" && event["params"]["delta"] == "streamed"
    }));
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "turn/completed")
    );
    Ok(())
}

#[test]
fn binary_run_executes_mock_tool_loop() -> Result<(), Box<dyn Error>> {
    let temp = temp_dir("openagent-cli-agent-loop")?;
    fs::write(temp.join("notes.txt"), "alpha\nbeta\n")?;
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
            "read",
            "notes",
        ])
        .env_clear()
        .env(
            "OPENAGENT_MOCK_TOOL_CALLS",
            r#"[{"call_id":"call_read","name":"read","input":{"file_path":"notes.txt"}}]"#,
        )
        .env("OPENAGENT_MOCK_ANSWER", "final answer")
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
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "item/toolCall/started")
    );
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "item/toolCall/completed")
    );
    let completed = events
        .iter()
        .find(|event| event["method"] == "turn/completed")
        .expect("completion event");
    assert_eq!(completed["params"]["final_answer"], "final answer");
    assert_eq!(completed["params"]["steps"], 2);
    assert_eq!(completed["params"]["tool_calls"], 1);

    let _ = fs::remove_dir_all(temp);
    Ok(())
}
