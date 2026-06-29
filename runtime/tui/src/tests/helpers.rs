fn send_key_text<H: TerminalEventHandler>(
    text: &str,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), Box<dyn Error>> {
    for ch in text.chars() {
        press_key(KeyCode::Char(ch), state, handler)?;
    }
    Ok(())
}

fn press_key<H: TerminalEventHandler>(
    key: KeyCode,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), Box<dyn Error>> {
    let exit = handle_key_event(KeyEvent::new(key, KeyModifiers::NONE), state, handler)
        .map_err(std::io::Error::other)?;
    assert!(!exit, "test key unexpectedly requested TUI exit");
    Ok(())
}

fn temp_test_dir(prefix: &str) -> Result<PathBuf, Box<dyn Error>> {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    let path = std::env::temp_dir().join(format!("{prefix}-{suffix}"));
    fs::create_dir_all(&path)?;
    Ok(path)
}

#[derive(Default)]
struct FakeBridgeState {
    requests: Vec<String>,
    turn_inputs: Vec<String>,
    approval_payloads: Vec<Value>,
    question_payloads: Vec<Value>,
    model_update_payloads: Vec<Value>,
    agent_update_payloads: Vec<Value>,
    variant_update_payloads: Vec<Value>,
    thinking_update_payloads: Vec<Value>,
    session_update_payloads: Vec<Value>,
}

struct FakeAppBridge {
    server_url: String,
    state: Arc<Mutex<FakeBridgeState>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl FakeAppBridge {
    fn start() -> Result<Self, Box<dyn Error>> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let state = Arc::new(Mutex::new(FakeBridgeState::default()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_shutdown = Arc::clone(&shutdown);
        let handle = thread::spawn(move || {
            while !thread_shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        let _ = handle_fake_bridge_connection(stream, &thread_state);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_error) => break,
                }
            }
        });
        Ok(Self {
            server_url: format!("http://127.0.0.1:{port}"),
            state,
            shutdown,
            handle: Some(handle),
        })
    }

    fn requests(&self) -> Vec<String> {
        self.state.lock().expect("bridge state").requests.clone()
    }

    fn turn_inputs(&self) -> Vec<String> {
        self.state.lock().expect("bridge state").turn_inputs.clone()
    }

    fn approval_payloads(&self) -> Vec<Value> {
        self.state
            .lock()
            .expect("bridge state")
            .approval_payloads
            .clone()
    }

    fn question_payloads(&self) -> Vec<Value> {
        self.state
            .lock()
            .expect("bridge state")
            .question_payloads
            .clone()
    }

    fn model_update_payloads(&self) -> Vec<Value> {
        self.state
            .lock()
            .expect("bridge state")
            .model_update_payloads
            .clone()
    }

    fn agent_update_payloads(&self) -> Vec<Value> {
        self.state
            .lock()
            .expect("bridge state")
            .agent_update_payloads
            .clone()
    }

    fn variant_update_payloads(&self) -> Vec<Value> {
        self.state
            .lock()
            .expect("bridge state")
            .variant_update_payloads
            .clone()
    }

    fn thinking_update_payloads(&self) -> Vec<Value> {
        self.state
            .lock()
            .expect("bridge state")
            .thinking_update_payloads
            .clone()
    }

    fn session_update_payloads(&self) -> Vec<Value> {
        self.state
            .lock()
            .expect("bridge state")
            .session_update_payloads
            .clone()
    }

    fn stop(mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.server_url.trim_start_matches("http://"));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_fake_bridge_connection(
    mut stream: TcpStream,
    state: &Arc<Mutex<FakeBridgeState>>,
) -> Result<(), Box<dyn Error>> {
    let (method, path, body) = read_http_request(&mut stream)?;
    state
        .lock()
        .expect("bridge state")
        .requests
        .push(format!("{method} {path}"));
    match (method.as_str(), path.as_str()) {
        ("GET", "/api/health") => write_json(&mut stream, json!({"ok": true})),
        ("GET", "/api/models") => write_json(&mut stream, fake_models_payload()),
        ("GET", "/api/agents") => write_json(&mut stream, fake_agents_payload()),
        ("GET", "/api/sessions") => write_json(&mut stream, json!({"sessions": []})),
        ("GET", "/api/sessions?query=smoke") => {
            write_json(&mut stream, fake_session_search_payload())
        }
        ("PATCH", "/api/sessions/session_smoke") => {
            let payload = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({}));
            state
                .lock()
                .expect("bridge state")
                .session_update_payloads
                .push(payload.clone());
            if payload.get("agent").is_some() {
                state
                    .lock()
                    .expect("bridge state")
                    .agent_update_payloads
                    .push(payload.clone());
            }
            if payload.get("model").is_some() {
                state
                    .lock()
                    .expect("bridge state")
                    .model_update_payloads
                    .push(payload.clone());
            }
            if payload.get("variant").is_some() {
                state
                    .lock()
                    .expect("bridge state")
                    .variant_update_payloads
                    .push(payload.clone());
            }
            if payload.get("thinking").is_some() {
                state
                    .lock()
                    .expect("bridge state")
                    .thinking_update_payloads
                    .push(payload.clone());
            }
            write_json(
                &mut stream,
                json!({
                    "session_id": "session_smoke",
                    "updated": true,
                    "session": {
                        "session_id": "session_smoke",
                        "id": "session_smoke",
                        "status": "idle",
                        "title": payload.get("title").cloned().unwrap_or(json!("Smoke Session")),
                        "archived": payload.get("archived").cloned().unwrap_or(json!(false)),
                        "message_count": 3,
                        "workspace": "/tmp/openagent-smoke",
                        "child_count": 1,
                        "model": payload.get("model").cloned().unwrap_or(Value::Null),
                        "agent": payload.get("agent").cloned().unwrap_or(Value::Null),
                        "variant": payload.get("variant").cloned().unwrap_or(Value::Null),
                        "thinking": payload.get("thinking").cloned().unwrap_or(Value::Null),
                        "metadata": {
                            "model": payload.get("model").cloned().unwrap_or(Value::Null),
                            "agent": payload.get("agent").cloned().unwrap_or(Value::Null),
                            "variant": payload.get("variant").cloned().unwrap_or(Value::Null),
                            "thinking": payload.get("thinking").cloned().unwrap_or(Value::Null)
                        }
                    }
                }),
            )
        }
        ("DELETE", "/api/sessions/session_smoke") => write_json(
            &mut stream,
            json!({
                "session_id": "session_smoke",
                "deleted": true
            }),
        ),
        ("GET", "/api/sessions/session_smoke") => write_json(
            &mut stream,
            json!({
                "session_id": "session_smoke",
                "session": {
                    "session_id": "session_smoke",
                    "title": "Smoke Session",
                    "status": "idle",
                    "metadata": {}
                }
            }),
        ),
        ("GET", "/api/sessions/session_smoke/children") => write_json(
            &mut stream,
            json!({
                "children": [{
                    "session_id": "session_child",
                    "title": "Smoke Child",
                    "status": "idle",
                    "message_count": 1,
                    "workspace": "/tmp/openagent-smoke",
                    "metadata": {"parent_session_id": "session_smoke"}
                }]
            }),
        ),
        ("POST", "/api/sessions/session_smoke/share") => write_json(
            &mut stream,
            json!({
                "session_id": "session_smoke",
                "shared": true,
                "share_url": "https://share.example/session_smoke"
            }),
        ),
        ("DELETE", "/api/sessions/session_smoke/share") => write_json(
            &mut stream,
            json!({
                "session_id": "session_smoke",
                "shared": false
            }),
        ),
        ("POST", "/api/sessions/session_smoke/compact") => write_json(
            &mut stream,
            json!({
                "session_id": "session_smoke",
                "summary": {"content": "compacted smoke session"}
            }),
        ),
        ("GET", "/api/sessions/session_smoke/diff") => write_json(
            &mut stream,
            json!({
                "session_id": "session_smoke",
                "files": [],
                "summary": {"changed": 0}
            }),
        ),
        ("POST", "/api/sessions/session_smoke/undo") => write_json(
            &mut stream,
            json!({
                "session_id": "session_smoke",
                "status": "ok",
                "events": []
            }),
        ),
        ("POST", "/api/sessions/session_smoke/redo") => write_json(
            &mut stream,
            json!({
                "session_id": "session_smoke",
                "status": "ok",
                "events": []
            }),
        ),
        ("GET", "/api/sessions/session_smoke/messages?limit=2") => {
            write_json(&mut stream, fake_transcript_payload())
        }
        ("POST", "/api/sessions") => write_json(
            &mut stream,
            json!({
                "session_id": "session_smoke",
                "session": {
                    "session_id": "session_smoke",
                    "status": "ready",
                    "message_count": 0
                }
            }),
        ),
        ("POST", "/api/sessions/session_smoke/turns") => {
            let input = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("input")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .unwrap_or_default();
            state.lock().expect("bridge state").turn_inputs.push(input);
            write_json(
                &mut stream,
                json!({
                    "session_id": "session_smoke",
                    "turn_id": "turn_smoke",
                    "status": "completed",
                    "events": fake_turn_events(),
                }),
            )
        }
        ("POST", "/api/turns/turn_approval/approvals/approval_smoke") => {
            let payload = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({}));
            state
                .lock()
                .expect("bridge state")
                .approval_payloads
                .push(payload);
            write_json(
                &mut stream,
                json!({
                    "session_id": "session_smoke",
                    "turn_id": "turn_approval",
                    "status": "completed",
                    "events": fake_approval_response_events(),
                }),
            )
        }
        ("POST", "/api/turns/turn_question/questions/question_smoke/reply") => {
            let payload = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({}));
            state
                .lock()
                .expect("bridge state")
                .question_payloads
                .push(payload);
            write_json(
                &mut stream,
                json!({
                    "session_id": "session_smoke",
                    "turn_id": "turn_question",
                    "status": "completed",
                    "events": fake_question_response_events(),
                }),
            )
        }
        _ if method == "GET" && path.starts_with("/api/events?last_event_id=") => {
            let last_event_id = path
                .rsplit_once('=')
                .and_then(|(_, value)| value.parse::<u64>().ok())
                .unwrap_or_default();
            let events = if last_event_id < 4 {
                vec![json!({
                    "method": "runtime/warning",
                    "global_sequence": 4,
                    "sequence": 4,
                    "params": {
                        "session_id": "session_smoke",
                        "turn_id": "turn_smoke",
                        "message": "bridge smoke warning"
                    }
                })]
            } else {
                Vec::new()
            };
            write_sse(&mut stream, &events)
        }
        _ => write_response(
            &mut stream,
            "404 Not Found",
            "application/json",
            &json!({"error": format!("unexpected route: {method} {path}")}).to_string(),
        ),
    }?;
    Ok(())
}

fn fake_turn_events() -> Vec<Value> {
    vec![
        json!({
            "method": "turn/started",
            "global_sequence": 1,
            "sequence": 1,
            "params": {
                "session_id": "session_smoke",
                "thread_id": "session_smoke",
                "turn_id": "turn_smoke",
                "status": "running"
            }
        }),
        json!({
            "method": "item/agentMessage/delta",
            "global_sequence": 2,
            "sequence": 2,
            "params": {
                "session_id": "session_smoke",
                "thread_id": "session_smoke",
                "turn_id": "turn_smoke",
                "delta": "bridge answer"
            }
        }),
        json!({
            "method": "turn/completed",
            "global_sequence": 3,
            "sequence": 3,
            "params": {
                "session_id": "session_smoke",
                "thread_id": "session_smoke",
                "turn_id": "turn_smoke",
                "status": "completed",
                "final_answer": "bridge answer",
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 2,
                    "total_tokens": 3,
                    "cost": 0.0
                }
            }
        }),
    ]
}

fn fake_models_payload() -> Value {
    json!({
        "models": [
            {
                "id": "server-local",
                "provider_id": "openagent",
                "name": "Server Local",
                "default": true
            },
            {
                "id": "deep-model",
                "provider_id": "openagent",
                "name": "Deep Model"
            }
        ],
        "variants": ["default", "deep"],
        "thinking": ["low", "high"]
    })
}

fn fake_agents_payload() -> Value {
    json!({
        "agents": [
            {
                "id": "server",
                "name": "Server",
                "description": "Default server-backed coding agent",
                "default": true
            },
            {
                "id": "reviewer",
                "name": "Reviewer",
                "description": "Review code"
            }
        ]
    })
}

fn fake_session_search_payload() -> Value {
    json!({
        "sessions": [{
            "session_id": "session_smoke",
            "title": "Smoke Session",
            "status": "idle",
            "message_count": 3,
            "workspace": "/tmp/openagent-smoke",
            "child_count": 1,
            "shared": false,
            "archived": false
        }]
    })
}

fn fake_transcript_payload() -> Value {
    json!({
        "session_id": "session_smoke",
        "message_count": 3,
        "limit": 2,
        "messages": [
            {
                "index": 1,
                "role": "assistant",
                "content": "bridge answer",
                "name": null,
                "tool_call_id": null,
                "metadata": {}
            },
            {
                "index": 2,
                "role": "user",
                "content": "next question",
                "name": null,
                "tool_call_id": null,
                "metadata": {}
            }
        ]
    })
}

fn fake_approval_response_events() -> Vec<Value> {
    vec![
        json!({
            "method": "turn/approval_resolved",
            "global_sequence": 10,
            "sequence": 10,
            "params": {
                "session_id": "session_smoke",
                "thread_id": "session_smoke",
                "turn_id": "turn_approval",
                "status": "running",
                "approval": {
                    "request_id": "approval_smoke",
                    "turn_id": "turn_approval",
                    "session_id": "session_smoke",
                    "tool_name": "bash",
                    "action": "allow",
                    "scope": "once"
                }
            }
        }),
        json!({
            "method": "turn/completed",
            "global_sequence": 11,
            "sequence": 11,
            "params": {
                "session_id": "session_smoke",
                "thread_id": "session_smoke",
                "turn_id": "turn_approval",
                "status": "completed",
                "final_answer": "approved through bridge"
            }
        }),
    ]
}

fn fake_question_response_events() -> Vec<Value> {
    vec![
        json!({
            "method": "item/question/resolved",
            "global_sequence": 12,
            "sequence": 12,
            "params": {
                "session_id": "session_smoke",
                "thread_id": "session_smoke",
                "turn_id": "turn_question",
                "status": "answered",
                "question": {
                    "request_id": "question_smoke",
                    "turn_id": "turn_question",
                    "session_id": "session_smoke"
                }
            }
        }),
        json!({
            "method": "turn/completed",
            "global_sequence": 13,
            "sequence": 13,
            "params": {
                "session_id": "session_smoke",
                "thread_id": "session_smoke",
                "turn_id": "turn_question",
                "status": "completed",
                "final_answer": "question bridge answer"
            }
        }),
    ]
}

fn read_http_request(stream: &mut TcpStream) -> Result<(String, String, String), Box<dyn Error>> {
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 1024];
    let mut expected_len = None;
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => raw.extend_from_slice(&buffer[..count]),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                break;
            }
            Err(error) => return Err(error.into()),
        }
        if expected_len.is_none()
            && let Some(header_end) = find_header_end(&raw)
        {
            let headers = String::from_utf8_lossy(&raw[..header_end]).to_string();
            let content_len = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or_default();
            expected_len = Some(header_end + content_len);
        }
        if let Some(expected_len) = expected_len
            && raw.len() >= expected_len
        {
            break;
        }
    }
    let header_end = find_header_end(&raw).ok_or("missing HTTP headers")?;
    let headers = String::from_utf8_lossy(&raw[..header_end]).to_string();
    let mut lines = headers.lines();
    let request_line = lines.next().ok_or("missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let body = String::from_utf8_lossy(&raw[header_end..]).to_string();
    Ok((method, path, body))
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn write_json(stream: &mut TcpStream, body: Value) -> Result<(), Box<dyn Error>> {
    write_response(stream, "200 OK", "application/json", &body.to_string())
}

fn write_sse(stream: &mut TcpStream, events: &[Value]) -> Result<(), Box<dyn Error>> {
    let mut body = String::new();
    for event in events {
        let method = event
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("event");
        let id = event
            .get("global_sequence")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        body.push_str(&format!("event: {method}\nid: {id}\ndata: {event}\n\n"));
    }
    write_response(stream, "200 OK", "text/event-stream", &body)
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<(), Box<dyn Error>> {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    Ok(())
}
