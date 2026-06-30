fn read_fixture() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/golden/rust_rewrite/cli_commands.json");
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn run_openagent<const N: usize>(
    args: [&str; N],
    cwd: Option<&Path>,
) -> Result<std::process::Output, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_openagent"));
    command.args(args).env_clear();
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    Ok(command.output()?)
}

fn run_openagent_vec(args: Vec<String>) -> Result<std::process::Output, Box<dyn Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args(args)
        .env_clear()
        .output()?)
}

fn spawn_openagent_server(
    port: u16,
    workspace: &Path,
    session_root: &Path,
) -> Result<Child, Box<dyn Error>> {
    let port = port.to_string();
    Ok(Command::new(env!("CARGO_BIN_EXE_openagent"))
        .args([
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            &port,
            "--workspace",
            path_str(workspace),
            "--session-root",
            path_str(session_root),
            "--auth-token",
            "secret",
        ])
        .env_clear()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?)
}

fn wait_for_attach(port: u16) -> Result<(), Box<dyn Error>> {
    let url = format!("http://127.0.0.1:{port}");
    for _ in 0..50 {
        let output = run_openagent_vec(vec![
            "attach".to_string(),
            url.clone(),
            "--server-token".to_string(),
            "secret".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ])?;
        if output.status.success() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err("server did not accept attach".into())
}

fn free_port() -> Result<u16, Box<dyn Error>> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

fn serve_http_once_on_free_port(
    content_type: &str,
    body: String,
) -> Result<(u16, MockServer), Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    Ok((
        port,
        serve_http_once_with_listener(listener, content_type, body),
    ))
}

fn serve_http_once_with_listener(
    listener: TcpListener,
    content_type: &str,
    body: String,
) -> MockServer {
    let content_type = content_type.to_string();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
        let _ = read_http_request_body(&mut stream)?;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .map_err(|error| error.to_string())
    })
}

fn serve_dripping_sse_provider() -> Result<DrippingSseProvider, Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let timed_out = Arc::new(AtomicBool::new(false));
    let server_timed_out = Arc::clone(&timed_out);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
        let _ = read_http_request_body(&mut stream)?;
        let first =
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hello \"},\"finish_reason\":null}]}\n\n";
        let second =
            b"data: {\"choices\":[{\"delta\":{\"content\":\"streamed\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n";
        let done = b"data: [DONE]\n\n";
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
            )
            .map_err(|error| error.to_string())?;
        write_http_chunk(&mut stream, first)?;
        stream.flush().map_err(|error| error.to_string())?;
        if release_rx.recv_timeout(Duration::from_secs(5)).is_err() {
            server_timed_out.store(true, Ordering::SeqCst);
        }
        write_http_chunk(&mut stream, second)?;
        write_http_chunk(&mut stream, done)?;
        stream
            .write_all(b"0\r\n\r\n")
            .map_err(|error| error.to_string())?;
        stream.flush().map_err(|error| error.to_string())
    });
    Ok((port, server, release_tx, timed_out))
}

fn write_http_chunk(stream: &mut std::net::TcpStream, chunk: &[u8]) -> Result<(), String> {
    stream
        .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
        .map_err(|error| error.to_string())?;
    stream.write_all(chunk).map_err(|error| error.to_string())?;
    stream.write_all(b"\r\n").map_err(|error| error.to_string())
}

fn serve_mcp_json_rpc(expected_requests: usize) -> Result<(u16, MockServer), Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let server = thread::spawn(move || {
        for _ in 0..expected_requests {
            let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
            let body = read_http_request_body(&mut stream)?;
            let request: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
            let method = request.get("method").and_then(Value::as_str).unwrap_or("");
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let response = if method == "tools/list" {
                json_response_body(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [{
                            "name": "echo",
                            "title": "Echo",
                            "description": "Echo text",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"text": {"type": "string"}},
                                "required": ["text"]
                            }
                        }]
                    }
                }))
            } else if method == "tools/call" {
                let text = request
                    .get("params")
                    .and_then(|params| params.get("arguments"))
                    .and_then(|arguments| arguments.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                json_response_body(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": format!("MCP echo {text}")}],
                        "isError": false
                    }
                }))
            } else {
                json_response_body(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": "method not found"}
                }))
            };
            stream
                .write_all(response.as_bytes())
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    });
    Ok((port, server))
}

fn stdio_mcp_server_script() -> &'static str {
    r#"import json
import sys


def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("utf-8").strip()
        if not line:
            break
        key, _, value = line.partition(":")
        headers[key.lower()] = value.strip()
    length = int(headers["content-length"])
    return json.loads(sys.stdin.buffer.read(length).decode("utf-8"))


def write_message(value):
    raw = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(raw))
    sys.stdout.buffer.write(raw)
    sys.stdout.buffer.flush()


while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        write_message({
            "jsonrpc": "2.0",
            "id": message.get("id"),
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "stdio-test", "version": "0.0.0"},
            },
        })
    elif method == "tools/list":
        write_message({
            "jsonrpc": "2.0",
            "id": message.get("id"),
            "result": {
                "tools": [{
                    "name": "arbor_review",
                    "title": "Arbor Review",
                    "description": "Review text",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"text": {"type": "string"}},
                        "required": ["text"],
                    },
                }],
            },
        })
    elif method == "tools/call":
        text = message.get("params", {}).get("arguments", {}).get("text", "")
        write_message({
            "jsonrpc": "2.0",
            "id": message.get("id"),
            "result": {
                "content": [{"type": "text", "text": "stdio MCP echo " + text}],
                "isError": False,
            },
        })
    elif method == "shutdown":
        write_message({"jsonrpc": "2.0", "id": message.get("id"), "result": {}})
    elif method == "exit":
        break
"#
}

fn read_http_request_body(stream: &mut std::net::TcpStream) -> Result<String, String> {
    let mut buffer = [0_u8; 8192];
    let read = stream
        .read(&mut buffer)
        .map_err(|error| error.to_string())?;
    let raw = String::from_utf8_lossy(&buffer[..read]).to_string();
    let (headers, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "invalid HTTP request".to_string())?;
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(body.len());
    let mut body = body.as_bytes().to_vec();
    while body.len() < content_length {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    body.truncate(content_length);
    String::from_utf8(body).map_err(|error| error.to_string())
}

fn json_response_body(value: Value) -> String {
    let body = value.to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
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

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap_or("")
}
