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

fn agents_payload() -> Value {
    json!({
        "agents": [
            {
                "id": "server",
                "name": "Server",
                "description": "Default server-backed coding agent",
                "default": true,
            },
            {
                "id": "coder",
                "name": "Coder",
                "description": "Implementation-focused profile",
            },
            {
                "id": "reviewer",
                "name": "Reviewer",
                "description": "Review and risk-focused profile",
            },
            {
                "id": "planner",
                "name": "Planner",
                "description": "Plan-first profile for large changes",
            }
        ],
    })
}

fn default_model_id() -> String {
    std::env::var("OPENAGENT_MODEL").unwrap_or_else(|_| "server-local".to_string())
}

fn mdns_payload(config: &HttpRuntimeConfig) -> Value {
    json!({
        "enabled": config.mdns_name.as_ref().is_some_and(|value| !value.is_empty()),
        "service": "_openagent._tcp",
        "name": config.mdns_name.clone().unwrap_or_default(),
        "host": config.host,
        "port": config.port,
        "url": format!("http://{}:{}", config.host, config.port),
    })
}

fn create_session_payload(config: &HttpRuntimeConfig, body: &str) -> Value {
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let workspace = payload
        .get("cwd")
        .or_else(|| payload.get("workspace"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| config.workspace.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let session_id = new_id("session");
    let store = FileSessionStore::new(session_root(config));
    let mut session = if let Some(fork_from) = payload.get("fork_from").and_then(Value::as_str) {
        store.load_session(fork_from).map_or_else(
            |_| Session::new(session_id.clone(), workspace.clone()),
            |base| {
                let mut forked = Session::new(session_id.clone(), workspace.clone());
                forked.messages = base.messages;
                forked.todos = base.todos;
                forked.metadata = base.metadata;
                forked
                    .metadata
                    .insert("forked_from".to_string(), json!(fork_from));
                forked
                    .metadata
                    .insert("parent_session_id".to_string(), json!(fork_from));
                forked
            },
        )
    } else {
        Session::new(session_id.clone(), workspace.clone())
    };
    session
        .metadata
        .insert("created_by".to_string(), json!("openagent-http-runtime"));
    if let Some(title) = payload
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        session
            .metadata
            .insert("title".to_string(), json!(title.trim()));
    }
    let _ = store.save_state(&session, None);
    json!({
        "session_id": session_id,
        "status": "created",
        "session": {
            "id": session_id,
            "session_id": session_id,
            "status": "idle",
            "message_count": 0,
            "workspace": workspace.to_string_lossy(),
        }
    })
}

fn get_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Value {
    let store = FileSessionStore::new(session_root(config));
    match store.load_session(session_id) {
        Ok(session) => json!({
            "session_id": session.id,
            "session": {
                "id": session.id,
                "session_id": session.id,
                "workspace": session.directory.to_string_lossy(),
                "status": session_status_text(&session.status),
                "message_count": session.messages.len(),
                "metadata": session.metadata,
            },
            "workspace": session.directory.to_string_lossy(),
            "status": session_status_text(&session.status),
            "message_count": session.messages.len(),
            "metadata": session.metadata,
        }),
        Err(error) => json!({"error": error.to_string()}),
    }
}

fn session_messages_payload(
    config: &HttpRuntimeConfig,
    session_id: &str,
    request_path: &str,
) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    let total = session.messages.len();
    let limit = query_param(request_path, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(50)
        .min(200);
    let before = query_param(request_path, "before");
    let messages_v2 = store
        .list_messages_with_parts(session_id, Some(limit), before.as_deref())
        .map_err(|error| error.to_string())?;
    let start = total.saturating_sub(limit);
    let messages = session
        .messages
        .iter()
        .enumerate()
        .skip(start)
        .map(|(index, message)| {
            let mut value = serde_json::to_value(message).unwrap_or_else(|_| json!({}));
            if let Some(object) = value.as_object_mut() {
                object.insert("index".to_string(), json!(index));
            }
            value
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "session_id": session.id,
        "message_count": total,
        "message_v2_count": messages_v2.len(),
        "limit": limit,
        "messages": messages,
        "messages_v2": messages_v2,
    }))
}

fn update_session_payload(
    config: &HttpRuntimeConfig,
    session_id: &str,
    body: &str,
) -> Result<Value, String> {
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    if let Some(title) = payload.get("title").and_then(Value::as_str) {
        let title = title.trim();
        if title.is_empty() {
            session.metadata.remove("title");
        } else {
            session.metadata.insert("title".to_string(), json!(title));
        }
    }
    if let Some(archived) = payload.get("archived").and_then(Value::as_bool) {
        if archived {
            session.metadata.insert("archived".to_string(), json!(true));
            session
                .metadata
                .insert("archived_at_ms".to_string(), json!(now_ms()));
        } else {
            session.metadata.remove("archived");
            session.metadata.remove("archived_at_ms");
        }
    }
    set_session_text_metadata(&mut session, &payload, "agent");
    set_session_text_metadata(&mut session, &payload, "model");
    set_session_text_metadata(&mut session, &payload, "variant");
    set_session_text_metadata(&mut session, &payload, "thinking");
    store
        .save_state(&session, None)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "session_id": session.id,
        "updated": true,
        "session": session_summary_from_session(&session),
    }))
}

fn delete_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    if !valid_session_id(session_id) {
        return Err("invalid session id".to_string());
    }
    let target = session_root(config).join(session_id);
    let removed = if target.exists() {
        fs::remove_dir_all(&target).map_err(|error| error.to_string())?;
        true
    } else {
        false
    };
    Ok(json!({"session_id": session_id, "removed": removed}))
}

fn session_children_payload(config: &HttpRuntimeConfig, session_id: &str) -> Value {
    let root = session_root(config);
    let mut children = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let state = read_json_file(&path.join("state.latest.json"));
            let parent = state
                .get("metadata")
                .and_then(|metadata| {
                    metadata
                        .get("parent_session_id")
                        .or_else(|| metadata.get("forked_from"))
                })
                .and_then(Value::as_str)
                .unwrap_or_default();
            if parent == session_id {
                children.push(session_summary_from_state(
                    &state,
                    &entry.file_name().to_string_lossy(),
                ));
            }
        }
    }
    children.sort_by(|left, right| {
        right["updated_at_ms"]
            .as_u64()
            .cmp(&left["updated_at_ms"].as_u64())
    });
    json!({"session_id": session_id, "children": children})
}

fn share_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    let share_id = session
        .metadata
        .get("share_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| new_id("share"));
    let url = format!("openagent://share/{share_id}");
    session.metadata.insert("shared".to_string(), json!(true));
    session
        .metadata
        .insert("share_id".to_string(), json!(share_id));
    session.metadata.insert("share_url".to_string(), json!(url));
    session
        .metadata
        .insert("shared_at_ms".to_string(), json!(now_ms()));
    store
        .save_state(&session, None)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "session_id": session.id,
        "shared": true,
        "share_id": session.metadata.get("share_id").cloned().unwrap_or(Value::Null),
        "url": session.metadata.get("share_url").cloned().unwrap_or(Value::Null),
    }))
}

fn unshare_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    session.metadata.remove("shared");
    session.metadata.remove("share_id");
    session.metadata.remove("share_url");
    session.metadata.remove("shared_at_ms");
    store
        .save_state(&session, None)
        .map_err(|error| error.to_string())?;
    Ok(json!({"session_id": session.id, "shared": false}))
}

fn compact_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    if matches!(
        session.status,
        SessionStatus::Running | SessionStatus::Paused
    ) {
        return Err("session must be idle before compacting".to_string());
    }
    session.status = SessionStatus::Compacting;
    store
        .save_state(&session, None)
        .map_err(|error| error.to_string())?;
    let summary = summarize_session_messages(&session);
    session.status = SessionStatus::Idle;
    session.metadata.insert(
        "compact".to_string(),
        json!({
            "compacted_at_ms": now_ms(),
            "message_count": session.messages.len(),
            "summary": summary,
        }),
    );
    store
        .save_state(&session, None)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "session_id": session.id,
        "status": "compacted",
        "summary": session.metadata.get("compact").cloned().unwrap_or(Value::Null),
    }))
}

fn session_diff_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    let undo_stack = file_change_stack(&session, FILE_CHANGE_UNDO_STACK_KEY);
    let redo_stack = file_change_stack(&session, FILE_CHANGE_REDO_STACK_KEY);
    let patches = undo_stack
        .iter()
        .rev()
        .map(public_file_change)
        .collect::<Vec<_>>();
    let redo = redo_stack
        .iter()
        .rev()
        .map(public_file_change)
        .collect::<Vec<_>>();
    Ok(json!({
        "session_id": session.id,
        "undo_count": undo_stack.len(),
        "redo_count": redo_stack.len(),
        "latest": undo_stack.last().map(public_file_change).unwrap_or(Value::Null),
        "patches": patches,
        "redo": redo,
    }))
}

fn undo_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    let mut undo_stack = file_change_stack(&session, FILE_CHANGE_UNDO_STACK_KEY);
    let Some(change) = undo_stack.pop() else {
        return Err("nothing to undo".to_string());
    };
    apply_file_change_state(&session, &change, FileChangeState::Before)?;
    let mut redo_stack = file_change_stack(&session, FILE_CHANGE_REDO_STACK_KEY);
    let reverted = mark_file_change(change.clone(), "undone");
    push_stack_entry(&mut redo_stack, reverted.clone());
    set_file_change_stack(&mut session, FILE_CHANGE_UNDO_STACK_KEY, undo_stack.clone());
    set_file_change_stack(&mut session, FILE_CHANGE_REDO_STACK_KEY, redo_stack.clone());
    session.metadata.insert(
        "latest_file_revert".to_string(),
        public_file_change(&reverted),
    );
    let turn_id = file_change_run_id(&change);
    let public = public_file_change(&reverted);
    let event = append_patch_stack_event(&store, &session, &turn_id, "patch/undone", &public);
    store
        .save_state(&session, Some(&turn_id))
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "session_id": session.id,
        "status": "undone",
        "undo_count": undo_stack.len(),
        "redo_count": redo_stack.len(),
        "patch": public,
        "events": [event],
    }))
}

fn redo_session_payload(config: &HttpRuntimeConfig, session_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .map_err(|error| error.to_string())?;
    let mut redo_stack = file_change_stack(&session, FILE_CHANGE_REDO_STACK_KEY);
    let Some(change) = redo_stack.pop() else {
        return Err("nothing to redo".to_string());
    };
    apply_file_change_state(&session, &change, FileChangeState::After)?;
    let mut undo_stack = file_change_stack(&session, FILE_CHANGE_UNDO_STACK_KEY);
    let reapplied = mark_file_change(change.clone(), "applied");
    push_stack_entry(&mut undo_stack, reapplied.clone());
    set_file_change_stack(&mut session, FILE_CHANGE_UNDO_STACK_KEY, undo_stack.clone());
    set_file_change_stack(&mut session, FILE_CHANGE_REDO_STACK_KEY, redo_stack.clone());
    session.metadata.insert(
        FILE_CHANGE_LATEST_KEY.to_string(),
        public_file_change(&reapplied),
    );
    let turn_id = file_change_run_id(&change);
    let public = public_file_change(&reapplied);
    let event = append_patch_stack_event(&store, &session, &turn_id, "patch/redone", &public);
    store
        .save_state(&session, Some(&turn_id))
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "session_id": session.id,
        "status": "redone",
        "undo_count": undo_stack.len(),
        "redo_count": redo_stack.len(),
        "patch": public,
        "events": [event],
    }))
}
