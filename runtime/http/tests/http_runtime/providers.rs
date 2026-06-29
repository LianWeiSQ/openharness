fn spawn_fake_openai_responses_provider() -> Result<(u16, thread::JoinHandle<()>), Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    Ok((
        port,
        thread::spawn(move || {
            if let Ok((mut stream, _addr)) = listener.accept() {
                let mut buffer = [0_u8; 8192];
                let _ = stream.read(&mut buffer);
                let body = serde_json::json!({
                    "id": "resp_fake",
                    "output_text": "real provider answer",
                    "usage": {"input_tokens": 7, "output_tokens": 3}
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }),
    ))
}

fn spawn_fake_openai_responses_provider_sequence(
    responses: Vec<Value>,
) -> Result<FakeProviderServer, Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    Ok((
        port,
        thread::spawn(move || {
            for body_value in responses {
                let Ok((mut stream, _addr)) = listener.accept() else {
                    break;
                };
                let mut buffer = [0_u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or_default();
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                if let Some((_, body)) = request.split_once("\r\n\r\n") {
                    if let Ok(mut items) = captured.lock() {
                        items.push(body.to_string());
                    }
                }
                let body = body_value.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }),
        requests,
    ))
}

fn spawn_fake_openai_responses_provider_sequence_with_delays(
    responses: Vec<(Value, u64)>,
) -> Result<FakeProviderServer, Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    Ok((
        port,
        thread::spawn(move || {
            for (body_value, delay_ms) in responses {
                let Ok((mut stream, _addr)) = listener.accept() else {
                    break;
                };
                let mut buffer = [0_u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or_default();
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                if let Some((_, body)) = request.split_once("\r\n\r\n") {
                    if let Ok(mut items) = captured.lock() {
                        items.push(body.to_string());
                    }
                }
                if delay_ms > 0 {
                    thread::sleep(Duration::from_millis(delay_ms));
                }
                let body = body_value.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }),
        requests,
    ))
}

fn spawn_fake_openai_responses_streaming_provider() -> Result<FakeProviderServer, Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    Ok((
        port,
        thread::spawn(move || {
            if let Ok((mut stream, _addr)) = listener.accept() {
                let mut buffer = [0_u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or_default();
                if let Ok(mut items) = captured.lock() {
                    items.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                }
                let headers = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\nconnection: close\r\n\r\n";
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(
                    b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"streamed \"}\n\n",
                );
                let _ = stream.flush();
                thread::sleep(Duration::from_millis(1500));
                let _ = stream.write_all(
                    b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"answer\"}\n\n",
                );
                let _ = stream.write_all(
                    b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":11,\"output_tokens\":2}}}\n\n",
                );
                let _ = stream.write_all(b"data: [DONE]\n\n");
                let _ = stream.flush();
            }
        }),
        requests,
    ))
}

fn spawn_fake_openai_responses_streaming_tool_then_delayed_final_provider()
-> Result<FakeProviderServer, Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    Ok((
        port,
        thread::spawn(move || {
            if let Ok((mut stream, _addr)) = listener.accept() {
                let mut buffer = [0_u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or_default();
                if let Ok(mut items) = captured.lock() {
                    items.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                }
                let headers = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\nconnection: close\r\n\r\n";
                let tool_event = serde_json::json!({
                    "type": "response.output_item.done",
                    "item": {
                        "type": "function_call",
                        "call_id": "call_live_read",
                        "name": "read",
                        "arguments": "{\"file_path\":\"notes.txt\"}",
                    }
                })
                .to_string();
                let usage_event = serde_json::json!({
                    "type": "response.completed",
                    "response": {"usage": {"input_tokens": 5, "output_tokens": 1}}
                })
                .to_string();
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(format!("data: {tool_event}\n\n").as_bytes());
                let _ = stream.write_all(format!("data: {usage_event}\n\n").as_bytes());
                let _ = stream.write_all(b"data: [DONE]\n\n");
                let _ = stream.flush();
            }

            if let Ok((mut stream, _addr)) = listener.accept() {
                let mut buffer = [0_u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or_default();
                if let Ok(mut items) = captured.lock() {
                    items.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                }
                thread::sleep(Duration::from_millis(1500));
                let body = serde_json::json!({
                    "id": "resp_final_after_tool",
                    "output_text": "tool final answer",
                    "usage": {"input_tokens": 12, "output_tokens": 3}
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }),
        requests,
    ))
}
