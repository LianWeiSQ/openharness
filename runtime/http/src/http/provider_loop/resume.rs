fn provider_max_steps(payload: &Value) -> u64 {
    payload
        .get("max_steps")
        .or_else(|| payload.get("maxSteps"))
        .and_then(Value::as_u64)
        .unwrap_or(4)
        .clamp(1, 16)
}

fn add_usage(total: &mut Usage, item: &Usage) {
    total.input_tokens = total.input_tokens.saturating_add(item.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(item.output_tokens);
    total.cost += item.cost;
}

fn provider_resume_payload(payload: &Value) -> Value {
    let mut value = payload.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("input");
        object.remove("message");
        object.remove("tool_call");
        object.remove("tool_calls");
        object.remove("api_key");
    }
    value
}

fn store_pending_provider_turn(
    session: &mut Session,
    payload: &Value,
    carry: &RuntimeProviderLoopCarry,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
) {
    session.metadata.insert(
        "pending_provider_turn".to_string(),
        json!({
            "payload": provider_resume_payload(payload),
            "answer": carry.answer.clone(),
            "usage": carry.usage.clone(),
            "tool_calls": carry.tool_calls,
            "next_step": carry.next_step,
            "permission": permission_ruleset.as_str(),
            "skip_permissions": skip_permissions,
        }),
    );
}

fn take_pending_provider_turn(session: &mut Session) -> Option<RuntimeProviderResume> {
    let pending = session.metadata.remove("pending_provider_turn")?;
    let permission_raw = pending
        .get("permission")
        .and_then(Value::as_str)
        .unwrap_or("FULL");
    Some(RuntimeProviderResume {
        payload: pending.get("payload").cloned().unwrap_or_else(|| json!({})),
        carry: RuntimeProviderLoopCarry {
            answer: pending
                .get("answer")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            usage: usage_from_provider_json(pending.get("usage")),
            tool_calls: pending
                .get("tool_calls")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            next_step: pending
                .get("next_step")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .max(1),
        },
        permission_ruleset: parse_permission_ruleset(permission_raw).ok()?,
        skip_permissions: pending
            .get("skip_permissions")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}
