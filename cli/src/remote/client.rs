use super::*;

pub(crate) fn remote_select_session(
    server_url: &str,
    token: Option<&str>,
    explicit: Option<String>,
    continue_last: bool,
    workspace: &Path,
) -> Result<String, String> {
    if let Some(session_id) = explicit {
        return Ok(session_id);
    }
    if continue_last {
        let payload = http_json("GET", server_url, "/api/sessions", token, None)?;
        if let Some(session_id) = payload
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("session_id").or_else(|| item.get("id")))
            .and_then(Value::as_str)
        {
            return Ok(session_id.to_string());
        }
    }
    let payload = http_json(
        "POST",
        server_url,
        "/api/sessions",
        token,
        Some(json!({"cwd": workspace.to_string_lossy()})),
    )?;
    payload
        .get("session_id")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "server did not return a session id".to_string())
}

pub(crate) fn remote_select_session_with_auth(
    server_url: &str,
    auth: &RemoteAuth,
    explicit: Option<String>,
    continue_last: bool,
    fork: bool,
    workspace: &Path,
) -> Result<String, String> {
    if fork && explicit.is_none() && !continue_last {
        return Err("--fork requires --continue or --session".to_string());
    }
    let base = if let Some(session_id) = explicit {
        Some(session_id)
    } else if continue_last {
        let payload = http_json_with_auth("GET", server_url, "/api/sessions", auth, None)?;
        payload
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("session_id").or_else(|| item.get("id")))
            .and_then(Value::as_str)
            .map(str::to_string)
    } else {
        None
    };
    if !fork && let Some(session_id) = base {
        return Ok(session_id);
    }
    let mut body = json!({"cwd": workspace.to_string_lossy()});
    if let Some(fork_from) = base {
        body["fork_from"] = json!(fork_from);
    }
    let payload = http_json_with_auth("POST", server_url, "/api/sessions", auth, Some(body))?;
    payload
        .get("session_id")
        .or_else(|| payload.get("id"))
        .or_else(|| payload.get("session").and_then(|session| session.get("id")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "server did not return a session id".to_string())
}

pub(crate) fn remote_start_turn(
    server_url: &str,
    token: Option<&str>,
    session_id: &str,
    prompt: &str,
) -> Result<Value, String> {
    http_json(
        "POST",
        server_url,
        &format!("/api/sessions/{session_id}/turns"),
        token,
        Some(json!({"input": prompt})),
    )
}

pub(crate) fn remote_start_turn_with_auth(
    server_url: &str,
    auth: &RemoteAuth,
    session_id: &str,
    prompt: &str,
) -> Result<Value, String> {
    http_json_with_auth(
        "POST",
        server_url,
        &format!("/api/sessions/{session_id}/turns"),
        auth,
        Some(json!({"input": prompt})),
    )
}

pub(super) fn remote_list_sessions(
    server_url: &str,
    auth: &RemoteAuth,
) -> Result<Vec<Value>, String> {
    let payload = http_json_with_auth("GET", server_url, "/api/sessions", auth, None)?;
    Ok(payload
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

pub(super) fn remote_tasks_payload(
    server_url: &str,
    auth: &RemoteAuth,
    session_id: &str,
) -> Result<Value, String> {
    http_json_with_auth(
        "GET",
        server_url,
        &format!("/api/sessions/{session_id}/tasks"),
        auth,
        None,
    )
}
