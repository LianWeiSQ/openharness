impl RemoteRuntimeClient {
    pub fn json(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let raw = self.text(method, path, body)?;
        serde_json::from_str(&raw).map_err(|error| format!("server response was not JSON: {error}"))
    }

    pub fn sse_events(&self, path: &str) -> Result<Vec<Value>, String> {
        let raw = self.text("GET", path, None)?;
        parse_sse_response_lines(&raw.lines().collect::<Vec<_>>())
    }

    pub fn text(&self, method: &str, path: &str, body: Option<Value>) -> Result<String, String> {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(self.timeout)
            .build()
            .map_err(|error| error.to_string())?;
        let url = join_server_url(&self.server_url, path);
        let mut request = match method {
            "DELETE" => client.delete(url),
            "GET" => client.get(url),
            "PATCH" => client.patch(url),
            "POST" => client.post(url),
            other => return Err(format!("unsupported HTTP method: {other}")),
        };
        if let Some(token) = self.auth.token.as_deref().filter(|value| !value.is_empty()) {
            request = request.bearer_auth(token);
        } else if let Some(password) = self
            .auth
            .password
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            request = request.basic_auth(
                self.auth.username.as_deref().unwrap_or("openagent"),
                Some(password),
            );
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().map_err(|error| {
            format!(
                "{method} {} failed: {error}",
                join_server_url(&self.server_url, path)
            )
        })?;
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let raw = response.text().map_err(|error| error.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "server returned HTTP {} for {method} {path}: {}",
                status.as_u16(),
                summarize_http_error_body(&raw, &content_type)
            ));
        }
        Ok(raw)
    }
}
