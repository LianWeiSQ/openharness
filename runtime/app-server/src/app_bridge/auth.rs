#[must_use]
pub fn is_authenticated_app_path(path: &str) -> bool {
    path.starts_with("/api/") || path.starts_with("/tui/")
}

#[must_use]
pub fn authorize_api_request(auth_token: Option<&str>, authorization: Option<&str>) -> bool {
    let Some(token) = auth_token.filter(|value| !value.is_empty()) else {
        return true;
    };
    authorization.is_some_and(|actual| actual == format!("Bearer {token}"))
}

#[must_use]
pub fn unauthorized_response_payload() -> Value {
    json!({
        "status": 401,
        "headers": {"WWW-Authenticate": UNAUTHORIZED_WWW_AUTHENTICATE},
        "json": {"error": "unauthorized"},
    })
}

#[must_use]
pub fn health_payload(serve_static: bool, auth_required: bool) -> Value {
    json!({
        "ok": true,
        "service": "openagent-app-server",
        "ui_enabled": serve_static,
        "auth_required": auth_required,
    })
}
