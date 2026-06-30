fn list_sessions_payload(config: &HttpRuntimeConfig, request_path: &str) -> Value {
    let root = session_root(config);
    let query = query_param(request_path, "query").unwrap_or_default();
    let mut sessions = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let state = read_json_file(&path.join("state.latest.json"));
            if state.as_object().is_none_or(Map::is_empty) {
                continue;
            }
            let summary = session_summary_from_state(&state, &entry.file_name().to_string_lossy());
            if !query.is_empty() && !session_matches_query(&summary, &query) {
                continue;
            }
            sessions.push(summary);
        }
    }
    sessions.sort_by(|left, right| {
        right["updated_at_ms"]
            .as_u64()
            .cmp(&left["updated_at_ms"].as_u64())
    });
    json!({"session_root": root.to_string_lossy(), "query": query, "sessions": sessions})
}

fn models_payload() -> Value {
    let current = default_model_id();
    let mut models = vec![json!({
        "id": current,
        "provider_id": "openagent",
        "name": "OpenAgent Server Local",
        "capabilities": {"tools": true, "streaming": true, "reasoning": true},
        "default": true,
    })];
    if models[0]["id"] != "server-local" {
        models.push(json!({
            "id": "server-local",
            "provider_id": "openagent",
            "name": "OpenAgent Server Local",
            "capabilities": {"tools": true, "streaming": true, "reasoning": true},
        }));
    }
    json!({
        "models": models,
        "variants": ["default", "fast", "balanced", "deep"],
        "thinking": ["off", "low", "medium", "high"],
    })
}
