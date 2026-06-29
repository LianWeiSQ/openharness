use super::*;

pub(super) fn session_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(session_help());
    }
    match args[0].as_str() {
        "list" | "ls" => session_list(&args[1..]),
        "export" => session_export(&args[1..]),
        "import" => session_import(&args[1..]),
        "share" => session_share(&args[1..]),
        "delete" | "rm" => session_delete(&args[1..]),
        _ => err_text(2, format!("unknown session command: {}", args[0])),
    }
}

pub(super) fn session_list(args: &[String]) -> CliRunResult {
    let root = session_root_from_args(args);
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
            let fallback_id = entry.file_name().to_string_lossy().to_string();
            sessions.push(json!({
                "session_id": state.get("session_id").and_then(Value::as_str).unwrap_or(&fallback_id),
                "workspace": state.get("workspace").cloned().unwrap_or_else(|| json!(".")),
                "status": state.get("status").cloned().unwrap_or_else(|| json!("idle")),
                "updated_at_ms": state.get("updated_at_ms").cloned().unwrap_or_else(|| json!(0)),
                "message_count": state.get("messages").and_then(Value::as_array).map_or(0, Vec::len),
            }));
        }
    }
    sessions.sort_by(|left, right| {
        right["updated_at_ms"]
            .as_u64()
            .cmp(&left["updated_at_ms"].as_u64())
    });
    if let Some(max_count) =
        value_for(args, &["--max-count", "-n"]).and_then(|value| value.parse::<usize>().ok())
    {
        sessions.truncate(max_count);
    }
    let payload = json!({"session_root": root.to_string_lossy(), "sessions": sessions});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        let sessions = payload["sessions"].as_array().cloned().unwrap_or_default();
        let mut text = render_key_values(
            "Sessions",
            &[
                ("Root", root.to_string_lossy().to_string()),
                ("Count", sessions.len().to_string()),
            ],
        );
        if !sessions.is_empty() {
            let rows = sessions
                .iter()
                .map(|session| {
                    vec![
                        session
                            .get("session_id")
                            .and_then(Value::as_str)
                            .unwrap_or("-")
                            .to_string(),
                        session
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("idle")
                            .to_string(),
                        session
                            .get("message_count")
                            .and_then(Value::as_u64)
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "0".to_string()),
                        session
                            .get("workspace")
                            .map(compact_text_value)
                            .unwrap_or_else(|| ".".to_string()),
                    ]
                })
                .collect::<Vec<_>>();
            text.push_str("\n\n");
            text.push_str(&render_table(
                &["Session", "Status", "Messages", "Workspace"],
                &rows,
            ));
        }
        ok_text(text)
    }
}

pub(super) fn session_export(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--session-root"]);
    let root = session_root_from_args(args);
    let session_id = if let Some(session_id) = positionals.first() {
        session_id.clone()
    } else {
        match latest_session_id(&root) {
            Some(session_id) => session_id,
            None => return err_text(2, "session export requires a session id"),
        }
    };
    if !valid_session_id(&session_id) {
        return err_text(2, "Invalid session id");
    }
    let state_path = root.join(&session_id).join("state.latest.json");
    let mut state = read_json_file(&state_path);
    if state.as_object().is_none_or(Map::is_empty) {
        return err_text(1, format!("Session state not found: {session_id}"));
    }
    if has_flag(args, &["--sanitize"]) {
        sanitize_session_state(&mut state);
    }
    CliRunResult::ok_json(
        &json!({"schema_version": "openagent.session_export.v1", "session": state}),
    )
}

pub(super) fn session_import(args: &[String]) -> CliRunResult {
    let positionals = positional_args(
        args,
        &["--workspace", "--dir", "--session-root", "--format"],
    );
    let Some(source) = positionals.first() else {
        return err_text(2, "session import requires a file or URL");
    };
    match import_session_source(&session_root_from_args(args), source) {
        Ok(payload) => CliRunResult::ok_json(&payload),
        Err(error) => err_text(1, error),
    }
}

fn session_share(args: &[String]) -> CliRunResult {
    let positionals = positional_args(
        args,
        &["--workspace", "--dir", "--session-root", "--format"],
    );
    let root = session_root_from_args(args);
    let session_id = if let Some(session_id) = positionals.first() {
        session_id.clone()
    } else {
        match latest_session_id(&root) {
            Some(session_id) => session_id,
            None => return err_text(2, "session share requires a session id"),
        }
    };
    let store = FileSessionStore::new(root);
    match share_session(&store, &session_id, has_flag(args, &["--sanitize"])) {
        Ok(payload) => CliRunResult::ok_json(&payload),
        Err(error) => err_text(1, error),
    }
}

fn session_delete(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--session-root"]);
    let Some(session_id) = positionals.first() else {
        return err_text(2, "session delete requires a session id");
    };
    if !valid_session_id(session_id) {
        return err_text(2, "Invalid session id");
    }
    let target = session_root_from_args(args).join(session_id);
    let removed = if target.exists() {
        match fs::remove_dir_all(&target) {
            Ok(()) => true,
            Err(error) => return err_text(1, format!("failed to delete session: {error}")),
        }
    } else {
        false
    };
    CliRunResult::ok_json(&json!({"session_id": session_id, "removed": removed}))
}

pub(super) fn latest_session_id(root: &Path) -> Option<String> {
    let mut sessions = fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let state = read_json_file(&path.join("state.latest.json"));
            let id = state
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| entry.file_name().to_string_lossy().to_string());
            let updated = state
                .get("updated_at_ms")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            Some((updated, id))
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|item| std::cmp::Reverse(item.0));
    sessions.into_iter().map(|(_, id)| id).next()
}

pub(super) fn share_session(
    store: &FileSessionStore,
    session_id: &str,
    sanitize: bool,
) -> Result<Value, String> {
    if !valid_session_id(session_id) {
        return Err("Invalid session id".to_string());
    }
    let state_path = store.root.join(session_id).join("state.latest.json");
    let mut state = read_json_file(&state_path);
    if state.as_object().is_none_or(Map::is_empty) {
        return Err(format!("Session state not found: {session_id}"));
    }
    if sanitize {
        sanitize_session_state(&mut state);
    }
    let share_dir = store.root.join("shares");
    fs::create_dir_all(&share_dir).map_err(|error| error.to_string())?;
    let share_id = new_cli_id("share");
    let path = share_dir.join(format!("{share_id}.json"));
    let payload = json!({
        "schema_version": "openagent.session_share.v1",
        "share_id": share_id,
        "session": state,
    });
    write_json_file(&path, &payload)?;
    Ok(json!({
        "share_id": payload["share_id"],
        "session_id": session_id,
        "path": path.to_string_lossy(),
        "url": format!("file://{}", path.to_string_lossy()),
    }))
}

fn import_session_source(root: &Path, source: &str) -> Result<Value, String> {
    let raw = if source.starts_with("http://") || source.starts_with("https://") {
        reqwest::blocking::get(source)
            .map_err(|error| format!("failed to fetch import source: {error}"))?
            .text()
            .map_err(|error| format!("failed to read import response: {error}"))?
    } else {
        fs::read_to_string(source)
            .map_err(|error| format!("failed to read import file: {error}"))?
    };
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("import source was not JSON: {error}"))?;
    let session = value
        .get("session")
        .cloned()
        .or_else(|| value.get("data").and_then(|data| data.get("session")).cloned())
        .or_else(|| {
            value
                .get("info")
                .map(|info| json!({"session_id": info.get("id").cloned().unwrap_or_else(|| json!(new_cli_id("session"))), "workspace": info.get("directory").cloned().unwrap_or_else(|| json!(".")), "status": "idle", "updated_at_ms": now_ms_cli(), "messages": value.get("messages").cloned().unwrap_or_else(|| json!([])), "metadata": {"imported_from": source}}))
        })
        .ok_or_else(|| "import source does not contain a session".to_string())?;
    let session_id = session
        .get("session_id")
        .or_else(|| session.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| new_cli_id("session"));
    if !valid_session_id(&session_id) {
        return Err("Invalid session id in import".to_string());
    }
    let target = root.join(&session_id);
    fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    let mut state = session;
    if let Some(object) = state.as_object_mut() {
        object.insert("session_id".to_string(), json!(session_id.clone()));
        object
            .entry("updated_at_ms".to_string())
            .or_insert_with(|| json!(now_ms_cli()));
        object
            .entry("schema_version".to_string())
            .or_insert_with(|| json!("openagent.session_state.v1"));
    }
    write_json_file(&target.join("state.latest.json"), &state)?;
    write_json_file(
        &target.join("session.json"),
        &json!({
            "schema_version": "openagent.session.v1",
            "session_id": session_id,
            "workspace": state.get("workspace").cloned().unwrap_or_else(|| json!(".")),
            "status": state.get("status").cloned().unwrap_or_else(|| json!("idle")),
            "created_at_ms": now_ms_cli(),
            "updated_at_ms": now_ms_cli(),
        }),
    )?;
    Ok(json!({"imported": true, "session_id": session_id, "session_root": root.to_string_lossy()}))
}
