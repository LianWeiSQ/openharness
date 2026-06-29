#[test]
fn binary_attach_and_tui_attach_use_remote_bridge_events() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-cli-attach")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_openagent_server(port, &workspace, &session_root)?;
    wait_for_attach(port)?;

    let url = format!("http://127.0.0.1:{port}");
    let run = run_openagent_vec(vec![
        "run".to_string(),
        "--attach".to_string(),
        url.clone(),
        "--server-token".to_string(),
        "secret".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "hello".to_string(),
        "attach".to_string(),
    ])?;
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let events = String::from_utf8(run.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "item/agentMessage/delta")
    );
    assert!(
        events
            .iter()
            .any(|event| event["method"] == "turn/completed")
    );

    let attach = run_openagent_vec(vec![
        "attach".to_string(),
        url.clone(),
        "--server-token".to_string(),
        "secret".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ])?;
    assert!(attach.status.success());
    let payload: Value = serde_json::from_slice(&attach.stdout)?;
    assert_eq!(payload["attached"], true);
    assert!(
        payload["sessions"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    let tui_attach = run_openagent_vec(vec![
        "tui".to_string(),
        "--attach".to_string(),
        url,
        "--server-token".to_string(),
        "secret".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ])?;
    assert!(tui_attach.status.success());
    let payload: Value = serde_json::from_slice(&tui_attach.stdout)?;
    assert_eq!(payload["attached"], true);

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}
