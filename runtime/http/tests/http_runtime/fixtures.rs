fn read_fixture() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/rust_rewrite/http_runtime.json");
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn section(fixture: &Value, name: &str) -> Value {
    fixture.get(name).cloned().unwrap_or(Value::Null)
}

fn run_live_interaction_resume_case(kind: &str) -> Result<(), Box<dyn Error>> {
    let first = match kind {
        "question" => serde_json::json!({
            "id": "resp_question_live",
            "output": [{
                "type": "function_call",
                "call_id": "call_question_live",
                "name": "question",
                "arguments": "{\"questions\":[{\"header\":\"Confirm\",\"question\":\"Proceed?\",\"multiple\":false,\"options\":[{\"label\":\"yes\",\"description\":\"Continue\"},{\"label\":\"no\",\"description\":\"Stop\"}]}]}"
            }],
            "usage": {"input_tokens": 4, "output_tokens": 1}
        }),
        "approval" => serde_json::json!({
            "id": "resp_approval_live",
            "output": [{
                "type": "function_call",
                "call_id": "call_bash_live",
                "name": "bash",
                "arguments": "{\"command\":\"printf approved\"}"
            }],
            "usage": {"input_tokens": 6, "output_tokens": 1}
        }),
        other => return Err(format!("unsupported interaction case: {other}").into()),
    };
    let final_answer = format!("{kind} final answer");
    let second = serde_json::json!({
        "id": format!("resp_{kind}_final"),
        "output_text": final_answer.clone(),
        "usage": {"input_tokens": 9, "output_tokens": 3}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence_with_delays(vec![
            (first, 0),
            (second, 1500),
        ])?;
    let port = free_port()?;
    let temp = temp_dir(&format!("openagent-http-runtime-live-{kind}-resume"))?;
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
    let started = if kind == "approval" {
        client.start_turn(
            &session_id,
            "run command with approval",
            serde_json::json!({"permission": "PLAN_ONLY"}),
        )?
    } else {
        client.start_turn(&session_id, "ask a question", serde_json::json!({}))?
    };
    assert_eq!(
        started["status"],
        if kind == "approval" {
            "waiting_approval"
        } else {
            "waiting_question"
        }
    );
    let mut response = if kind == "approval" {
        Value::Object(
            started["events"]
                .as_array()
                .expect("events")
                .iter()
                .find(|event| event["method"] == "turn/approval_requested")
                .and_then(|event| event["params"]["approval"].as_object())
                .cloned()
                .expect("approval event"),
        )
    } else {
        Value::Object(
            started["events"]
                .as_array()
                .expect("events")
                .iter()
                .find(|event| event["method"] == "item/question/requested")
                .and_then(|event| event["params"]["event"].as_object())
                .cloned()
                .expect("question event"),
        )
    };
    if kind == "approval" {
        response["action"] = Value::String("allow".to_string());
        response["scope"] = Value::String("once".to_string());
    } else {
        response["answers"] = serde_json::json!([["yes"]]);
    }
    let request_id = response["request_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

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

    let response_for_thread = response.clone();
    let kind_for_thread = kind.to_string();
    let reply = thread::spawn(move || {
        let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
            .with_auth(RemoteAuth::bearer("secret"));
        if kind_for_thread == "approval" {
            client
                .respond_approval(&response_for_thread)
                .map_err(|error| error.to_string())
        } else {
            client
                .respond_question(&response_for_thread)
                .map_err(|error| error.to_string())
        }
    });

    let live_response = live
        .join()
        .map_err(|_| "live sse thread panicked".to_string())?
        .map_err(|error| format!("live sse request failed: {error}"))?;
    if kind == "approval" {
        assert!(live_response.contains("event: turn/approval_resolved"));
        assert!(live_response.contains("running"));
    } else {
        assert!(live_response.contains("event: item/question/resolved"));
        assert!(live_response.contains("answered"));
    }
    assert!(live_response.contains(&request_id));
    assert!(live_response.contains(&session_id));
    assert!(!live_response.contains("event: turn/completed"));

    let resolved = reply
        .join()
        .map_err(|_| "interaction reply thread panicked".to_string())?
        .map_err(|error| format!("interaction reply failed: {error}"))?;
    assert_eq!(resolved["status"], "completed");
    assert_eq!(resolved["turn"]["final_answer"], final_answer);

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("function_call_output"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

fn free_port() -> Result<u16, Box<dyn Error>> {
    static NEXT_PORT: AtomicU16 = AtomicU16::new(0);
    if NEXT_PORT.load(Ordering::Relaxed) == 0 {
        let seed = 20_000 + (std::process::id() % 20_000) as u16;
        let _ = NEXT_PORT.compare_exchange(0, seed, Ordering::Relaxed, Ordering::Relaxed);
    }
    for _ in 0..10_000 {
        let port = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

fn temp_dir(prefix: &str) -> Result<PathBuf, Box<dyn Error>> {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    let path = std::env::temp_dir().join(format!("{prefix}-{suffix}"));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn spawn_runtime(
    port: u16,
    workspace: &std::path::Path,
    session_root: &std::path::Path,
) -> Result<Child, Box<dyn Error>> {
    spawn_runtime_with_env(port, workspace, session_root, &[])
}

fn spawn_runtime_with_env(
    port: u16,
    workspace: &std::path::Path,
    session_root: &std::path::Path,
    envs: &[(&str, &str)],
) -> Result<Child, Box<dyn Error>> {
    let port = port.to_string();
    let mut command = Command::new(env!("CARGO_BIN_EXE_openagent-http-runtime"));
    command
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port,
            "--workspace",
            workspace.to_str().unwrap_or("."),
            "--session-root",
            session_root.to_str().unwrap_or("."),
            "--auth-token",
            "secret",
            "--username",
            "openagent",
            "--password",
            "pass",
            "--cors-origin",
            "http://client.test",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in envs {
        command.env(key, value);
    }
    Ok(command.spawn()?)
}
