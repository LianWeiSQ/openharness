pub fn parse_turn_approval_path(path: &str) -> Result<(String, String), String> {
    let raw = path.strip_prefix("/api/turns/").unwrap_or(path);
    let Some((turn_id, request_id)) = raw.split_once("/approvals/") else {
        return Err(
            "approval path must be /api/turns/{turn_id}/approvals/{request_id}".to_string(),
        );
    };
    let turn_id = turn_id.trim_matches('/');
    let request_id = request_id.trim_matches('/');
    if turn_id.is_empty() || request_id.is_empty() {
        return Err(
            "approval path must be /api/turns/{turn_id}/approvals/{request_id}".to_string(),
        );
    }
    Ok((turn_id.to_string(), request_id.to_string()))
}

pub fn parse_turn_question_reply_path(path: &str) -> Result<(String, String), String> {
    let raw = path.strip_prefix("/api/turns/").unwrap_or(path);
    let Some((turn_id, rest)) = raw.split_once("/questions/") else {
        return Err(
            "question reply path must be /api/turns/{turn_id}/questions/{request_id}/reply"
                .to_string(),
        );
    };
    let Some(request_id) = rest.strip_suffix("/reply") else {
        return Err(
            "question reply path must be /api/turns/{turn_id}/questions/{request_id}/reply"
                .to_string(),
        );
    };
    let turn_id = turn_id.trim_matches('/');
    let request_id = request_id.trim_matches('/');
    if turn_id.is_empty() || request_id.is_empty() {
        return Err(
            "question reply path must be /api/turns/{turn_id}/questions/{request_id}/reply"
                .to_string(),
        );
    }
    Ok((turn_id.to_string(), request_id.to_string()))
}

pub fn approval_response_payload(payload: &Value) -> Result<Value, String> {
    let raw_action = required_string(payload, "action")?;
    let action = normalize_approval_action(&raw_action)?;
    let scope = normalize_approval_scope(optional_string(payload, "scope").as_deref())?
        .or_else(|| implied_approval_scope(&raw_action));
    let note = optional_string(payload, "note");
    let mut object = Map::new();
    object.insert("action".to_string(), Value::String(action));
    if let Some(scope) = scope {
        object.insert("scope".to_string(), Value::String(scope));
    }
    if let Some(note) = note {
        object.insert("note".to_string(), Value::String(note));
    }
    Ok(Value::Object(object))
}

pub fn question_reply_payload(payload: &Value) -> Result<Value, String> {
    let answers = normalized_question_answers(payload)?;
    let mut object = Map::new();
    object.insert("answers".to_string(), answers);
    object.insert("dismissed".to_string(), Value::Bool(false));
    if let Some(note) = optional_string(payload, "note") {
        object.insert("note".to_string(), Value::String(note));
    }
    Ok(Value::Object(object))
}

pub fn question_dismiss_payload(payload: &Value) -> Value {
    let mut object = Map::new();
    object.insert("answers".to_string(), Value::Array(Vec::new()));
    object.insert("dismissed".to_string(), Value::Bool(true));
    if let Some(note) = optional_string(payload, "note") {
        object.insert("note".to_string(), Value::String(note));
    }
    Value::Object(object)
}

fn normalize_approval_action(action: &str) -> Result<String, String> {
    match action.trim().to_ascii_lowercase().as_str() {
        "allow" | "approve" | "approved" | "yes" | "y" | "allow_once" | "allow-once" => {
            Ok("allow".to_string())
        }
        "allow_always" | "allow-always" | "always" => Ok("allow".to_string()),
        "deny" | "reject" | "rejected" | "no" | "n" => Ok("deny".to_string()),
        _ => Err("approval action must be allow or deny".to_string()),
    }
}

fn normalize_approval_scope(scope: Option<&str>) -> Result<Option<String>, String> {
    match scope.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(None),
        Some("once") | Some("this") => Ok(Some("once".to_string())),
        Some("always") | Some("session") => Ok(Some("always".to_string())),
        Some(_) => Err("approval scope must be once or always".to_string()),
    }
}

fn implied_approval_scope(action: &str) -> Option<String> {
    match action.trim().to_ascii_lowercase().as_str() {
        "allow_always" | "allow-always" | "always" => Some("always".to_string()),
        "allow_once" | "allow-once" => Some("once".to_string()),
        _ => None,
    }
}

fn normalized_question_answers(payload: &Value) -> Result<Value, String> {
    if let Some(answers) = payload.get("answers") {
        return normalize_answer_array(answers);
    }
    if let Some(answer) = optional_string(payload, "answer") {
        return Ok(json!([[answer]]));
    }
    Err("answers or answer is required".to_string())
}

fn normalize_answer_array(value: &Value) -> Result<Value, String> {
    let Some(items) = value.as_array() else {
        return Err("answers must be an array".to_string());
    };
    let normalized = items
        .iter()
        .map(|item| {
            if let Some(text) = item.as_str() {
                if text.trim().is_empty() {
                    Err("answers must contain non-empty strings".to_string())
                } else {
                    Ok(json!([text]))
                }
            } else if let Some(values) = item.as_array() {
                let answers = values
                    .iter()
                    .map(|answer| {
                        answer
                            .as_str()
                            .filter(|value| !value.trim().is_empty())
                            .map(|value| Value::String(value.to_string()))
                            .ok_or_else(|| "answers must contain non-empty strings".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if answers.is_empty() {
                    return Err("answers must contain at least one string".to_string());
                }
                Ok(Value::Array(answers))
            } else {
                Err("answers must contain strings or string arrays".to_string())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    if normalized.is_empty() {
        return Err("answers must not be empty".to_string());
    }
    Ok(Value::Array(normalized))
}
