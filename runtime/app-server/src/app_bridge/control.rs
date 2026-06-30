pub fn publish_to_control(payload: &Value) -> Result<(String, Value), String> {
    let topic = ["type", "topic", "event", "method"]
        .iter()
        .find_map(|key| payload.get(*key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "publish type is required".to_string())?;
    let raw_payload = payload.get("properties").or_else(|| payload.get("payload"));
    let params = if let Some(Value::Object(object)) = raw_payload {
        Value::Object(object.clone())
    } else {
        let mut object = Map::new();
        if let Some(source) = payload.as_object() {
            for (key, value) in source {
                if !matches!(
                    key.as_str(),
                    "type" | "topic" | "event" | "method" | "properties" | "payload"
                ) {
                    object.insert(key.clone(), value.clone());
                }
            }
        }
        Value::Object(object)
    };

    match topic {
        "tui.prompt.append" => Ok((
            "prompt.append".to_string(),
            json!({"text": required_string(&params, "text")?}),
        )),
        "tui.command.execute" => Ok((
            "command.execute".to_string(),
            json!({"command": required_string(&params, "command")?}),
        )),
        "tui.toast.show" => {
            let mut result = Map::new();
            result.insert(
                "message".to_string(),
                Value::String(required_string(&params, "message")?),
            );
            if let Some(object) = params.as_object() {
                for key in ["title", "variant", "duration"] {
                    if let Some(value) = object.get(key) {
                        if !value.is_null() {
                            result.insert(key.to_string(), value.clone());
                        }
                    }
                }
            }
            Ok(("toast.show".to_string(), Value::Object(result)))
        }
        "tui.session.select" => Ok((
            "session.select".to_string(),
            json!({"sessionID": required_string(&params, "sessionID")?}),
        )),
        "tui.approval.respond" => Ok((
            "approval.respond".to_string(),
            approval_response_payload(&params)?,
        )),
        "tui.question.reply" => Ok(("question.reply".to_string(), question_reply_payload(&params)?)),
        "tui.question.dismiss" => Ok((
            "question.dismiss".to_string(),
            question_dismiss_payload(&params),
        )),
        _ => Err(format!("unsupported publish type: {topic}")),
    }
}

pub fn tui_control_request_for_path(
    path: &str,
    payload: &Value,
) -> Result<TuiControlRequest, String> {
    let body = match path {
        "/tui/append-prompt" => json!({"text": required_string(payload, "text")?}),
        "/tui/submit-prompt" | "/tui/clear-prompt" | "/tui/open-help" | "/tui/open-sessions"
        | "/tui/open-themes" | "/tui/open-models" => json!({}),
        "/tui/execute-command" => json!({"command": required_string(payload, "command")?}),
        "/tui/show-toast" => validate_toast_payload(payload)?,
        "/tui/select-session" => json!({"sessionID": required_string(payload, "sessionID")?}),
        "/tui/respond-approval" => approval_response_payload(payload)?,
        "/tui/reply-question" => question_reply_payload(payload)?,
        "/tui/dismiss-question" => question_dismiss_payload(payload),
        "/tui/publish" => {
            let _ = publish_to_control(payload)?;
            payload.clone()
        }
        _ => return Err("unknown endpoint".to_string()),
    };
    Ok(TuiControlRequest::new(path, body))
}

#[must_use]
pub fn control_next_payload(request: Option<&TuiControlRequest>) -> Value {
    request.map_or_else(
        || json!({"path": "", "body": null}),
        TuiControlRequest::to_value,
    )
}

#[must_use]
pub fn record_control_response_payload(payload: Value) -> Value {
    json!({"ok": true, "response": payload})
}

fn validate_toast_payload(payload: &Value) -> Result<Value, String> {
    let message = required_string(payload, "message")?;
    let mut object = Map::new();
    object.insert("message".to_string(), Value::String(message));
    for key in ["title", "variant"] {
        if let Some(value) = payload.get(key) {
            if !value.is_null() {
                if !value.is_string() {
                    return Err(format!("{key} must be a string"));
                }
                object.insert(key.to_string(), value.clone());
            }
        }
    }
    if let Some(value) = payload.get("duration") {
        if !value.is_null() {
            if !value.is_number() {
                return Err("duration must be a number".to_string());
            }
            object.insert("duration".to_string(), value.clone());
        }
    }
    Ok(Value::Object(object))
}
