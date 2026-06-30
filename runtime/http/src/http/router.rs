fn handle_http_stream(stream: &mut TcpStream, config: &HttpRuntimeConfig) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let request = read_http_request(stream)?;
    if should_live_sse(&request, config) {
        return write_live_sse_response(stream, config, &request);
    }
    let response = route_http_request(&request, config);
    write_http_response(stream, with_runtime_headers(response, config))
}

#[derive(Clone, Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: String,
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 1024 * 1024 {
            return Err("request headers too large".to_string());
        }
    }
    let split = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .ok_or_else(|| "invalid HTTP request".to_string())?;
    let head = String::from_utf8_lossy(&buffer[..split]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let mut body_bytes = buffer[split..].to_vec();
    while body_bytes.len() < content_length {
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        body_bytes.extend_from_slice(&chunk[..read]);
    }
    body_bytes.truncate(content_length);
    Ok(HttpRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body_bytes).to_string(),
    })
}

fn route_http_request(request: &HttpRequest, config: &HttpRuntimeConfig) -> HttpResponseSpec {
    if request.method == "OPTIONS" {
        return route_options();
    }
    if !authorized(request, config) {
        return route_unauthorized();
    }
    let path = request.path.split('?').next().unwrap_or("/");
    match (request.method.as_str(), path) {
        ("GET", "/api/health") => json_response(200, health_payload(config)),
        ("GET", "/api/models") => json_response(200, models_payload()),
        ("GET", "/api/agents") => json_response(200, agents_payload(config)),
        ("GET", "/api/mdns") => json_response(200, mdns_payload(config)),
        ("GET", "/api/events") => sse_response(global_sse_frames(config, &request.path)),
        ("GET", "/api/sessions") => {
            json_response(200, list_sessions_payload(config, &request.path))
        }
        ("POST", "/api/sessions") => {
            json_response(200, create_session_payload(config, &request.body))
        }
        ("GET", "/") if config.serve_static => {
            static_response("text/html; charset=utf-8", INDEX_HTML)
        }
        ("GET", "/index.html") if config.serve_static => {
            static_response("text/html; charset=utf-8", INDEX_HTML)
        }
        ("GET", "/app.js") if config.serve_static => {
            static_response("application/javascript; charset=utf-8", APP_JS)
        }
        ("GET", "/app.css") if config.serve_static => {
            static_response("text/css; charset=utf-8", APP_CSS)
        }
        _ => route_dynamic_request(request, config, path),
    }
}

fn route_dynamic_request(
    request: &HttpRequest,
    config: &HttpRuntimeConfig,
    path: &str,
) -> HttpResponseSpec {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.len() == 3 && parts[0] == "api" && parts[1] == "sessions" {
        return match request.method.as_str() {
            "GET" => json_response(200, get_session_payload(config, parts[2])),
            "PATCH" => match update_session_payload(config, parts[2], &request.body) {
                Ok(payload) => json_response(200, payload),
                Err(error) => json_response(400, json!({"error": error})),
            },
            "DELETE" => match delete_session_payload(config, parts[2]) {
                Ok(payload) => json_response(200, payload),
                Err(error) => json_response(400, json!({"error": error})),
            },
            _ => route_unknown(),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "messages"
        && request.method == "GET"
    {
        return match session_messages_payload(config, parts[2], &request.path) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "children"
        && request.method == "GET"
    {
        return json_response(200, session_children_payload(config, parts[2]));
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "tasks"
        && request.method == "GET"
    {
        return json_response(200, session_tasks_payload(config, parts[2]));
    }
    if parts.len() == 6
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "tasks"
        && parts[5] == "run"
        && request.method == "POST"
    {
        return match run_session_task_payload(config, parts[2], parts[4], &request.body) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 6
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "tasks"
        && parts[5] == "cancel"
        && request.method == "POST"
    {
        return match cancel_session_task_payload(config, parts[2], parts[4]) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 4 && parts[0] == "api" && parts[1] == "sessions" && parts[3] == "share" {
        return match request.method.as_str() {
            "POST" => match share_session_payload(config, parts[2]) {
                Ok(payload) => json_response(200, payload),
                Err(error) => json_response(400, json!({"error": error})),
            },
            "DELETE" => match unshare_session_payload(config, parts[2]) {
                Ok(payload) => json_response(200, payload),
                Err(error) => json_response(400, json!({"error": error})),
            },
            _ => route_unknown(),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "compact"
        && request.method == "POST"
    {
        return match compact_session_payload(config, parts[2]) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 4 && parts[0] == "api" && parts[1] == "sessions" && parts[3] == "diff" {
        return match request.method.as_str() {
            "GET" => match session_diff_payload(config, parts[2]) {
                Ok(payload) => json_response(200, payload),
                Err(error) => json_response(400, json!({"error": error})),
            },
            _ => route_unknown(),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "undo"
        && request.method == "POST"
    {
        return match undo_session_payload(config, parts[2]) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "redo"
        && request.method == "POST"
    {
        return match redo_session_payload(config, parts[2]) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "sessions"
        && parts[3] == "turns"
        && request.method == "POST"
    {
        return match start_turn_payload(config, parts[2], &request.body) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "turns"
        && parts[3] == "events"
        && request.method == "GET"
    {
        return sse_response(turn_sse_frames(config, parts[2], &request.path));
    }
    if parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "turns"
        && parts[3] == "interrupt"
        && request.method == "POST"
    {
        return match interrupt_turn_payload(config, parts[2]) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if path.starts_with("/api/turns/") && path.contains("/approvals/") && request.method == "POST" {
        return match respond_approval_payload(config, path, &request.body) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if path.starts_with("/api/turns/")
        && path.contains("/questions/")
        && path.ends_with("/reply")
        && request.method == "POST"
    {
        return match respond_question_payload(config, path, &request.body) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    if path == "/tui/control/next" && request.method == "GET" {
        return json_response(200, pop_tui_control_payload(config));
    }
    if path == "/tui/control/response" && request.method == "POST" {
        return json_response(200, record_tui_control_response(config, &request.body));
    }
    if path.starts_with("/tui/") && request.method == "POST" {
        return match enqueue_tui_control_payload(config, path, &request.body) {
            Ok(payload) => json_response(200, payload),
            Err(error) => json_response(400, json!({"error": error})),
        };
    }
    route_unknown()
}

fn authorized(request: &HttpRequest, config: &HttpRuntimeConfig) -> bool {
    if !config.auth_required() {
        return true;
    }
    if let Some(token) = config.auth_token.as_ref().filter(|token| !token.is_empty()) {
        let bearer_ok = request
            .headers
            .get("authorization")
            .is_some_and(|value| value == &format!("Bearer {token}"));
        let header_ok = request
            .headers
            .get("x-openagent-token")
            .is_some_and(|value| value == token);
        if bearer_ok || header_ok {
            return true;
        }
    }
    basic_auth_ok(
        request.headers.get("authorization").map(String::as_str),
        config,
    )
}

fn json_response(status: u16, body: Value) -> HttpResponseSpec {
    HttpResponseSpec {
        status,
        content_type: Some("application/json; charset=utf-8".to_string()),
        headers: Map::new(),
        body: Some(body),
        body_text: None,
    }
}

fn static_response(content_type: &str, body: &str) -> HttpResponseSpec {
    HttpResponseSpec {
        status: 200,
        content_type: Some(content_type.to_string()),
        headers: Map::new(),
        body: None,
        body_text: Some(body.to_string()),
    }
}

fn sse_response(body: String) -> HttpResponseSpec {
    let mut headers = Map::new();
    headers.insert("Cache-Control".to_string(), json!("no-cache"));
    headers.insert("X-Accel-Buffering".to_string(), json!("no"));
    HttpResponseSpec {
        status: 200,
        content_type: Some("text/event-stream; charset=utf-8".to_string()),
        headers,
        body: None,
        body_text: Some(body),
    }
}

fn should_live_sse(request: &HttpRequest, config: &HttpRuntimeConfig) -> bool {
    if request.method != "GET" || !authorized(request, config) {
        return false;
    }
    let path = request.path.split('?').next().unwrap_or("/");
    let is_sse_path = path == "/api/events" || turn_id_from_events_path(path).is_some();
    is_sse_path
        && request
            .headers
            .get("accept")
            .is_some_and(|value| value.contains("text/event-stream"))
}

fn write_live_sse_response(
    stream: &mut TcpStream,
    config: &HttpRuntimeConfig,
    request: &HttpRequest,
) -> Result<(), String> {
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let path = request.path.split('?').next().unwrap_or("/");
    let turn_id = turn_id_from_events_path(path);
    let headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\ncache-control: no-cache, no-transform\r\nx-accel-buffering: no\r\naccess-control-allow-origin: {}\r\nconnection: close\r\n\r\n",
        config.cors_origin
    );
    stream
        .write_all(headers.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut last_id = last_event_id_from_path(&request.path);
    let timeout = live_sse_timeout(&request.path);
    let started = Instant::now();
    let mut last_heartbeat = Instant::now();
    loop {
        let mut terminal_seen = false;
        for (id, event) in live_sse_events_after(config, turn_id.as_deref(), last_id) {
            stream
                .write_all(sse_frame(id, &event).as_bytes())
                .map_err(|error| error.to_string())?;
            last_id = id;
            if is_terminal_turn_event(&event) {
                terminal_seen = true;
            }
        }
        stream.flush().map_err(|error| error.to_string())?;
        if terminal_seen {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            stream
                .write_all(b": ping\n\n")
                .map_err(|error| error.to_string())?;
            stream.flush().map_err(|error| error.to_string())?;
            return Ok(());
        }
        if last_heartbeat.elapsed() >= Duration::from_secs(10) {
            stream
                .write_all(b": ping\n\n")
                .map_err(|error| error.to_string())?;
            stream.flush().map_err(|error| error.to_string())?;
            last_heartbeat = Instant::now();
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn live_sse_events_after(
    config: &HttpRuntimeConfig,
    turn_id: Option<&str>,
    last_id: u64,
) -> Vec<(u64, Value)> {
    let events = if let Some(turn_id) = turn_id {
        turn_app_events(config, turn_id)
    } else {
        all_app_events(config)
    };
    events
        .into_iter()
        .enumerate()
        .filter_map(|(index, event)| {
            let id = event
                .get(if turn_id.is_some() {
                    "sequence"
                } else {
                    "global_sequence"
                })
                .or_else(|| event.get("sequence"))
                .and_then(Value::as_u64)
                .unwrap_or(index as u64 + 1);
            (id > last_id).then_some((id, event))
        })
        .collect()
}

fn turn_id_from_events_path(path: &str) -> Option<String> {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    (parts.len() == 4 && parts[0] == "api" && parts[1] == "turns" && parts[3] == "events")
        .then(|| parts[2].to_string())
}

fn live_sse_timeout(request_path: &str) -> Duration {
    let millis = query_value(request_path, "live_timeout_ms")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30_000)
        .clamp(250, 300_000);
    Duration::from_millis(millis)
}

fn is_terminal_turn_event(event: &Value) -> bool {
    matches!(
        event.get("method").and_then(Value::as_str),
        Some("turn/completed" | "turn/failed" | "turn/interrupted")
    )
}

fn with_runtime_headers(
    mut response: HttpResponseSpec,
    config: &HttpRuntimeConfig,
) -> HttpResponseSpec {
    response.headers.insert(
        "Access-Control-Allow-Origin".to_string(),
        json!(config.cors_origin.clone()),
    );
    response.headers.insert(
        "Access-Control-Allow-Headers".to_string(),
        json!("Authorization, Content-Type, X-OpenAgent-Token"),
    );
    response.headers.insert(
        "Access-Control-Allow-Methods".to_string(),
        json!("GET, POST, PATCH, DELETE, OPTIONS"),
    );
    response
}

fn write_http_response(stream: &mut TcpStream, response: HttpResponseSpec) -> Result<(), String> {
    let body = response.body_text.unwrap_or_else(|| {
        response
            .body
            .as_ref()
            .map(stable_json_dumps)
            .unwrap_or_default()
    });
    let content_type = response
        .content_type
        .unwrap_or_else(|| "application/json; charset=utf-8".to_string());
    let status_text = match response.status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "OK",
    };
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n",
        response.status,
        status_text,
        content_type,
        body.len()
    );
    for (key, value) in response.headers {
        if let Some(value) = value.as_str() {
            headers.push_str(&format!("{key}: {value}\r\n"));
        }
    }
    headers.push_str("\r\n");
    stream
        .write_all(headers.as_bytes())
        .and_then(|()| stream.write_all(body.as_bytes()))
        .map_err(|error| error.to_string())
}

fn basic_auth_ok(authorization: Option<&str>, config: &HttpRuntimeConfig) -> bool {
    let Some(password) = config
        .auth_password
        .as_ref()
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let username = config.auth_username.as_deref().unwrap_or("openagent");
    let Some(encoded) = authorization.and_then(|value| value.strip_prefix("Basic ")) else {
        return false;
    };
    decode_base64(encoded).is_some_and(|decoded| decoded == format!("{username}:{password}"))
}

fn decode_base64(value: &str) -> Option<String> {
    let mut output = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u8;
    for byte in value.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let sextet = base64_value(byte)? as u32;
        buffer = (buffer << 6) | sextet;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    String::from_utf8(output).ok()
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}
