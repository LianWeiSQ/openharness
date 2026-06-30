use super::*;

pub(super) fn http_json(
    method: &str,
    server_url: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Result<Value, String> {
    http_json_with_auth(
        method,
        server_url,
        path,
        &RemoteAuth {
            token: token.map(str::to_string),
            username: None,
            password: None,
        },
        body,
    )
}

pub(super) fn http_json_with_auth(
    method: &str,
    server_url: &str,
    path: &str,
    auth: &RemoteAuth,
    body: Option<Value>,
) -> Result<Value, String> {
    let raw = http_text_with_auth(method, server_url, path, auth, body)?;
    serde_json::from_str(&raw).map_err(|error| format!("server response was not JSON: {error}"))
}

pub(super) fn http_text_with_auth(
    method: &str,
    server_url: &str,
    path: &str,
    auth: &RemoteAuth,
    body: Option<Value>,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let url = format!("{}{}", server_url.trim_end_matches('/'), path);
    let mut request = match method {
        "GET" => client.get(url),
        "POST" => client.post(url),
        _ => return Err(format!("unsupported HTTP method: {method}")),
    };
    if let Some(token) = auth.token.as_deref().filter(|value| !value.is_empty()) {
        request = request.bearer_auth(token);
    }
    if let Some(password) = auth.password.as_deref().filter(|value| !value.is_empty()) {
        request = request.basic_auth(
            auth.username.as_deref().unwrap_or("openagent"),
            Some(password),
        );
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .map_err(|error| format!("{method} {path} failed: {error}"))?;
    let status = response.status();
    let raw = response.text().map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "{method} {path} returned HTTP {}: {raw}",
            status.as_u16()
        ));
    }
    Ok(raw)
}
