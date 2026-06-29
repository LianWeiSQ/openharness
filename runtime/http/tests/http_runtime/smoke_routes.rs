#[test]
fn http_runtime_fixture_matches_legacy_oracle() -> Result<(), Box<dyn Error>> {
    let fixture = read_fixture()?;
    assert_eq!(http_runtime_fixture(), fixture);
    Ok(())
}

#[test]
fn binary_health_json_smoke_matches_docker_contract() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_openagent-http-runtime"))
        .arg("--health-json")
        .output()?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        payload,
        section(&read_fixture()?, "docker")["expected_stdout_json"]
    );
    Ok(())
}

#[test]
fn dockerfile_matches_smoke_contract() -> Result<(), Box<dyn Error>> {
    let dockerfile = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Dockerfile.openagent-http-runtime"),
    )?;
    let lines = dockerfile
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(Value::from)
        .collect::<Vec<_>>();
    assert_eq!(
        Value::Array(lines),
        section(&read_fixture()?, "docker")["dockerfile"]
    );
    Ok(())
}

#[test]
fn app_bridge_http_routes_cover_static_sse_auth_and_tui_control() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-routes")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let unauthorized = http_request(port, "GET", "/api/health", &[], "")?;
    assert!(unauthorized.starts_with("HTTP/1.1 401"));
    assert!(unauthorized.contains("WWW-Authenticate: Bearer"));

    let basic = http_request(
        port,
        "GET",
        "/api/health",
        &[("Authorization", "Basic b3BlbmFnZW50OnBhc3M=")],
        "",
    )?;
    assert_eq!(json_body(&basic)?["ok"], true);

    let index = authorized_request(port, "GET", "/", "", true)?;
    assert!(index.contains("content-type: text/html"));
    assert!(index.contains("OpenAgent"));

    let created = json_body(&authorized_request(
        port,
        "POST",
        "/api/sessions",
        &format!("{{\"cwd\":\"{}\"}}", workspace.to_string_lossy()),
        false,
    )?)?;
    let session_id = created["session_id"].as_str().expect("session id");
    let started = json_body(&authorized_request(
        port,
        "POST",
        &format!("/api/sessions/{session_id}/turns"),
        "{\"input\":\"hello over bridge\"}",
        false,
    )?)?;
    let turn_id = started["turn_id"].as_str().expect("turn id");

    let turn_events = authorized_request(
        port,
        "GET",
        &format!("/api/turns/{turn_id}/events"),
        "",
        true,
    )?;
    assert!(turn_events.contains("content-type: text/event-stream"));
    assert!(turn_events.contains("event: item/agentMessage/delta"));
    assert!(turn_events.contains("event: turn/completed"));

    let global_events = authorized_request(port, "GET", "/api/events?last_event_id=0", "", true)?;
    assert!(global_events.contains("event: turn/completed"));

    let interrupted = json_body(&authorized_request(
        port,
        "POST",
        &format!("/api/turns/{turn_id}/interrupt"),
        "",
        false,
    )?)?;
    assert_eq!(interrupted["status"], "interrupted");

    let queued = json_body(&authorized_request(
        port,
        "POST",
        "/tui/append-prompt",
        "{\"text\":\"queued prompt\"}",
        false,
    )?)?;
    assert_eq!(queued["queued"], true);
    let next = json_body(&authorized_request(
        port,
        "GET",
        "/tui/control/next",
        "",
        false,
    )?)?;
    assert_eq!(next["path"], "/tui/append-prompt");
    assert_eq!(next["body"]["text"], "queued prompt");

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}
