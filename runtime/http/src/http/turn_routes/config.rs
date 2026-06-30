fn permission_ruleset_for_turn(payload: &Value) -> Result<PermissionRuleset, String> {
    let raw = payload
        .get("permission")
        .or_else(|| payload.get("permissions"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| std::env::var("OPENAGENT_APP_PERMISSION").ok())
        .unwrap_or_else(|| "FULL".to_string());
    parse_permission_ruleset(&raw)
}

fn parse_permission_ruleset(raw: &str) -> Result<PermissionRuleset, String> {
    match raw.trim().to_ascii_uppercase().replace('-', "_").as_str() {
        "FULL" | "ALLOW" | "AUTO" => Ok(PermissionRuleset::Full),
        "READONLY" | "READ_ONLY" => Ok(PermissionRuleset::Readonly),
        "PLAN_ONLY" | "ASK" => Ok(PermissionRuleset::PlanOnly),
        "NONE" | "DENY" => Ok(PermissionRuleset::None),
        _ => Err("permission must be FULL, READONLY, PLAN_ONLY, or NONE".to_string()),
    }
}

fn skip_permissions_for_turn(payload: &Value) -> bool {
    payload
        .get("dangerously_skip_permissions")
        .or_else(|| payload.get("skip_permissions"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            std::env::var("OPENAGENT_APP_DANGEROUSLY_SKIP_PERMISSIONS")
                .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes"))
        })
}

fn provider_streaming_enabled_for_turn(payload: &Value) -> bool {
    payload
        .get("stream")
        .or_else(|| payload.get("provider_stream"))
        .or_else(|| payload.get("stream_provider"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            std::env::var("OPENAGENT_PROVIDER_STREAM")
                .map(|value| !matches!(value.as_str(), "0" | "false" | "FALSE" | "no"))
                .unwrap_or(true)
        })
}
