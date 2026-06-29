#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeProfile {
    agent: String,
    model: String,
    variant: String,
    thinking: String,
}

fn apply_turn_runtime_profile(session: &mut Session, payload: &Value) -> RuntimeProfile {
    set_session_text_metadata(session, payload, "agent");
    set_session_text_metadata(session, payload, "model");
    set_session_text_metadata(session, payload, "variant");
    set_session_text_metadata(session, payload, "thinking");
    let profile = RuntimeProfile {
        agent: session_text_metadata(session, "agent", "server"),
        model: session_text_metadata(session, "model", &default_model_id()),
        variant: session_text_metadata(session, "variant", "default"),
        thinking: session_text_metadata(session, "thinking", "medium"),
    };
    session
        .metadata
        .insert("agent".to_string(), json!(profile.agent.clone()));
    session
        .metadata
        .insert("model".to_string(), json!(profile.model.clone()));
    session
        .metadata
        .insert("variant".to_string(), json!(profile.variant.clone()));
    session
        .metadata
        .insert("thinking".to_string(), json!(profile.thinking.clone()));
    profile
}

fn set_session_text_metadata(session: &mut Session, payload: &Value, key: &str) {
    let Some(value) = payload.get(key).and_then(Value::as_str) else {
        return;
    };
    let value = value.trim();
    if value.is_empty() {
        session.metadata.remove(key);
    } else {
        session.metadata.insert(key.to_string(), json!(value));
    }
}

fn session_text_metadata(session: &Session, key: &str, default: &str) -> String {
    session
        .metadata
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| default.to_string())
}

fn turn_started_event(session: &Session, run_id: &str) -> Value {
    let profile = RuntimeProfile {
        agent: session_text_metadata(session, "agent", "server"),
        model: session_text_metadata(session, "model", &default_model_id()),
        variant: session_text_metadata(session, "variant", "default"),
        thinking: session_text_metadata(session, "thinking", "medium"),
    };
    json!({
        "method": "turn/started",
        "params": {
            "session_id": session.id,
            "thread_id": session.id,
            "turn_id": run_id,
            "status": "running",
            "agent": profile.agent,
            "agent_name": profile.agent,
            "model": profile.model,
            "model_id": profile.model,
            "provider_id": "openagent",
            "variant": profile.variant,
            "thinking": profile.thinking,
        },
    })
}

fn latest_user_message(session: &Session) -> String {
    session
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, Role::User))
        .map(|message| message.content.clone())
        .unwrap_or_default()
}

fn usage_payload(input: &str, output: &str, tool_calls: u64) -> Value {
    let input_tokens = estimate_tokens(input);
    let output_tokens = estimate_tokens(output);
    let tool_tokens = tool_calls.saturating_mul(16);
    let total_tokens = input_tokens + output_tokens + tool_tokens;
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "tool_tokens": tool_tokens,
        "total_tokens": total_tokens,
        "tool_calls": tool_calls,
        "cost": 0.0,
        "estimated": true,
    })
}

fn estimate_tokens(value: &str) -> u64 {
    let by_words = value.split_whitespace().count() as u64;
    let by_chars = (value.chars().count() as u64).div_ceil(4);
    by_words.max(by_chars).max(u64::from(!value.is_empty()))
}

fn trace_payload(session: &Session, run_id: &str, tool_calls: u64) -> Value {
    json!({
        "run_id": run_id,
        "session_id": session.id,
        "agent": session_text_metadata(session, "agent", "server"),
        "model": session_text_metadata(session, "model", &default_model_id()),
        "variant": session_text_metadata(session, "variant", "default"),
        "thinking": session_text_metadata(session, "thinking", "medium"),
        "tool_calls": tool_calls,
    })
}

fn record_usage_event(store: &FileSessionStore, session: &Session, run_id: &str, usage: &Value) {
    let _ = store.record_event(
        &session.id,
        run_id,
        "model.usage",
        SessionEventOptions {
            kind: "usage".to_string(),
            attributes: usage
                .as_object()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            ..SessionEventOptions::default()
        },
    );
}

fn tool_calls_completed_successfully(events: &[Value]) -> bool {
    events
        .iter()
        .any(|event| event.get("method").and_then(Value::as_str) == Some("item/toolCall/completed"))
        && !events.iter().any(|event| {
            event.get("method").and_then(Value::as_str) == Some("item/toolCall/failed")
        })
}
