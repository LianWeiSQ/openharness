fn find_session_with_pending_approval(
    store: &FileSessionStore,
    turn_id: &str,
    request_id: &str,
) -> Result<Session, String> {
    for entry in fs::read_dir(&store.root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.path().is_dir() {
            continue;
        }
        let state = read_json_file(&entry.path().join("state.latest.json"));
        let Some(pending) = state
            .get("metadata")
            .and_then(|metadata| metadata.get("pending_approval"))
        else {
            continue;
        };
        let same_turn = pending
            .get("turn_id")
            .or_else(|| pending.get("run_id"))
            .and_then(Value::as_str)
            == Some(turn_id);
        let same_request = pending.get("request_id").and_then(Value::as_str) == Some(request_id);
        if same_turn && same_request {
            let session_id = state
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "pending approval session is missing session_id".to_string())?;
            return store
                .load_session(session_id)
                .map_err(|error| error.to_string());
        }
    }
    Err("pending approval not found".to_string())
}

fn find_session_with_pending_question(
    store: &FileSessionStore,
    turn_id: &str,
    request_id: &str,
) -> Result<Session, String> {
    for entry in fs::read_dir(&store.root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.path().is_dir() {
            continue;
        }
        let state = read_json_file(&entry.path().join("state.latest.json"));
        let Some(pending) = state
            .get("metadata")
            .and_then(|metadata| metadata.get("pending_question"))
        else {
            continue;
        };
        let same_turn = pending
            .get("turn_id")
            .or_else(|| pending.get("run_id"))
            .and_then(Value::as_str)
            == Some(turn_id);
        let same_request = pending.get("request_id").and_then(Value::as_str) == Some(request_id);
        if same_turn && same_request {
            let session_id = state
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "pending question session is missing session_id".to_string())?;
            return store
                .load_session(session_id)
                .map_err(|error| error.to_string());
        }
    }
    Err("pending question not found".to_string())
}
