fn wait_for_server(port: u16) -> Result<(), Box<dyn Error>> {
    for _ in 0..100 {
        if authorized_request(port, "GET", "/api/health", "", false).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("server did not start".into())
}

fn wait_for_task_status(
    client: &RemoteRuntimeClient,
    session_id: &str,
    task_id: &str,
    expected: &str,
) -> Result<Value, Box<dyn Error>> {
    for _ in 0..100 {
        let tasks = client.tasks(session_id)?;
        if let Some(task) = tasks.iter().find(|task| task["session_id"] == task_id)
            && task["status"] == expected
        {
            return Ok(task.clone());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!("task {task_id} did not reach status {expected}").into())
}

fn authorized_request(
    port: u16,
    method: &str,
    path: &str,
    body: &str,
    raw: bool,
) -> Result<String, Box<dyn Error>> {
    let response = http_request(
        port,
        method,
        path,
        &[("Authorization", "Bearer secret")],
        body,
    )?;
    if raw || response.starts_with("HTTP/1.1 2") {
        return Ok(response);
    }
    Err(format!("request failed: {response}").into())
}

fn http_request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<String, Box<dyn Error>> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if !body.is_empty() {
        request.push_str("Content-Type: application/json\r\n");
    }
    for (key, value) in headers {
        request.push_str(&format!("{key}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn json_body(response: &str) -> Result<Value, Box<dyn Error>> {
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(response);
    Ok(serde_json::from_str(body)?)
}
