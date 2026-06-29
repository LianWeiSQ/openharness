#[must_use]
pub fn health_payload(config: &HttpRuntimeConfig) -> Value {
    json!({
        "ok": true,
        "service": command_name(),
        "app_bridge": app_server_crate_name(),
        "ui_enabled": config.serve_static,
        "auth_required": config.auth_required(),
    })
}

#[must_use]
pub fn route_health() -> HttpResponseSpec {
    HttpResponseSpec {
        status: 200,
        content_type: Some("application/json; charset=utf-8".to_string()),
        headers: Map::new(),
        body: None,
        body_text: None,
    }
}

#[must_use]
pub fn route_unauthorized() -> HttpResponseSpec {
    let mut headers = Map::new();
    headers.insert(
        "WWW-Authenticate".to_string(),
        Value::String(
            "Bearer realm=\"openagent-app-bridge\", Basic realm=\"openagent-app-bridge\""
                .to_string(),
        ),
    );
    HttpResponseSpec {
        status: 401,
        content_type: None,
        headers,
        body: Some(json!({"error": "unauthorized"})),
        body_text: None,
    }
}

#[must_use]
pub fn route_options() -> HttpResponseSpec {
    let mut headers = Map::new();
    headers.insert(
        "Access-Control-Allow-Methods".to_string(),
        Value::String("GET, POST, PATCH, DELETE, OPTIONS".to_string()),
    );
    headers.insert(
        "Access-Control-Allow-Headers".to_string(),
        Value::String("Authorization, Content-Type, X-OpenAgent-Token".to_string()),
    );
    headers.insert(
        "Access-Control-Max-Age".to_string(),
        Value::String("600".to_string()),
    );
    HttpResponseSpec {
        status: 204,
        content_type: None,
        headers,
        body: None,
        body_text: None,
    }
}

#[must_use]
pub fn route_unknown() -> HttpResponseSpec {
    HttpResponseSpec {
        status: 404,
        content_type: None,
        headers: Map::new(),
        body: Some(json!({"error": "unknown endpoint"})),
        body_text: None,
    }
}

pub fn parse_sse_response_lines(lines: &[&str]) -> Result<Vec<Value>, String> {
    let mut events = Vec::new();
    let mut data_lines: Vec<String> = Vec::new();
    for raw_line in lines {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if !data_lines.is_empty() {
                events.push(parse_sse_data(&data_lines.join("\n"))?);
                data_lines.clear();
            }
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }
    if !data_lines.is_empty() {
        events.push(parse_sse_data(&data_lines.join("\n"))?);
    }
    Ok(events)
}

pub fn parse_sse_data(data: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(data).map_err(|error| error.to_string())?;
    if !value.is_object() {
        return Err("SSE event data was not a JSON object".to_string());
    }
    Ok(value)
}

#[must_use]
pub fn format_http_error(method: &str, path: &str, code: u16, body: Option<&Value>) -> String {
    if let Some(error) = body
        .and_then(|value| value.get("error"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return format!("{method} {path} returned HTTP {code}: {error}");
    }
    format!("{method} {path} returned HTTP {code}")
}
