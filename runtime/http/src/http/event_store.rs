fn append_app_events(root: &Path, session_id: &str, turn_id: &str, events: &[Value]) {
    let path = app_events_path(root, session_id, turn_id);
    let existing = read_jsonl_values(&path).len() as u64;
    for (index, event) in events.iter().enumerate() {
        let mut normalized = event.clone();
        if let Some(object) = normalized.as_object_mut() {
            object
                .entry("sequence".to_string())
                .or_insert_with(|| json!(existing + index as u64 + 1));
            object
                .entry("created_at_ms".to_string())
                .or_insert_with(|| json!(now_ms()));
            object
                .entry("global_sequence".to_string())
                .or_insert_with(|| json!(existing + index as u64 + 1));
        }
        append_json_line(&path, &normalized);
    }
}

fn append_unpersisted_app_events(
    root: &Path,
    session_id: &str,
    turn_id: &str,
    events: &[Value],
    persisted_events: &mut usize,
) {
    if *persisted_events >= events.len() {
        return;
    }
    append_app_events(root, session_id, turn_id, &events[*persisted_events..]);
    *persisted_events = events.len();
}

fn global_sse_frames(config: &HttpRuntimeConfig, request_path: &str) -> String {
    let last_id = last_event_id_from_path(request_path);
    let mut frames = String::new();
    for (index, event) in all_app_events(config).into_iter().enumerate() {
        let id = event
            .get("global_sequence")
            .or_else(|| event.get("sequence"))
            .and_then(Value::as_u64)
            .unwrap_or(index as u64 + 1);
        if id <= last_id {
            continue;
        }
        frames.push_str(&sse_frame(id, &event));
    }
    if frames.is_empty() {
        frames.push_str(": ping\n\n");
    }
    frames
}

fn turn_sse_frames(config: &HttpRuntimeConfig, turn_id: &str, request_path: &str) -> String {
    let last_id = last_event_id_from_path(request_path);
    let mut frames = String::new();
    for event in turn_app_events(config, turn_id) {
        let id = event
            .get("sequence")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if id <= last_id {
            continue;
        }
        frames.push_str(&sse_frame(id, &event));
    }
    if frames.is_empty() {
        frames.push_str(": ping\n\n");
    }
    frames
}

fn sse_frame(id: u64, event: &Value) -> String {
    let event_name = event
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("message");
    format!(
        "id: {id}\nevent: {event_name}\ndata: {}\n\n",
        stable_json_dumps(event)
    )
}

fn all_app_events(config: &HttpRuntimeConfig) -> Vec<Value> {
    let root = session_root(config);
    let mut events = Vec::new();
    if let Ok(sessions) = fs::read_dir(&root) {
        for session in sessions.flatten() {
            let runs_dir = session.path().join("runs");
            if let Ok(runs) = fs::read_dir(runs_dir) {
                for run in runs.flatten() {
                    events.extend(read_jsonl_values(&run.path().join(APP_EVENTS_FILE)));
                }
            }
        }
    }
    events.sort_by_key(|event| {
        event
            .get("created_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    });
    for (index, event) in events.iter_mut().enumerate() {
        if let Some(object) = event.as_object_mut() {
            object.insert("global_sequence".to_string(), json!(index as u64 + 1));
        }
    }
    events
}

fn turn_app_events(config: &HttpRuntimeConfig, turn_id: &str) -> Vec<Value> {
    let root = session_root(config);
    if let Ok(sessions) = fs::read_dir(&root) {
        for session in sessions.flatten() {
            let path = app_events_path(&root, &session.file_name().to_string_lossy(), turn_id);
            if path.exists() {
                return read_jsonl_values(&path);
            }
        }
    }
    Vec::new()
}

fn app_events_path(root: &Path, session_id: &str, turn_id: &str) -> PathBuf {
    root.join(session_id)
        .join("runs")
        .join(turn_id)
        .join(APP_EVENTS_FILE)
}

fn last_event_id_from_path(path: &str) -> u64 {
    query_value(path, "last_event_id")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}

fn query_value(path: &str, name: &str) -> Option<String> {
    path.split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.replace("%20", " ").replace('+', " ")))
}

fn find_session_for_turn(
    store: &FileSessionStore,
    turn_id: &str,
) -> Result<(String, Session), String> {
    for entry in fs::read_dir(&store.root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.path().join("runs").join(turn_id).is_dir() {
            continue;
        }
        let session_id = entry.file_name().to_string_lossy().to_string();
        let session = store
            .load_session(&session_id)
            .map_err(|error| error.to_string())?;
        return Ok((session_id, session));
    }
    Err("turn not found".to_string())
}

fn tui_control_queue_path(config: &HttpRuntimeConfig) -> PathBuf {
    session_root(config).join(TUI_CONTROL_QUEUE_FILE)
}

fn tui_control_responses_path(config: &HttpRuntimeConfig) -> PathBuf {
    session_root(config).join(TUI_CONTROL_RESPONSES_FILE)
}

fn read_json_array(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
}

fn read_jsonl_values(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .ok()
        .map(|raw| {
            raw.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn write_json_value(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, stable_json_dumps(value)).map_err(|error| error.to_string())
}

fn append_json_line(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{}", stable_json_dumps(value));
    }
}

fn session_root(config: &HttpRuntimeConfig) -> PathBuf {
    config
        .session_store_root
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace(config).join(".openagent/sessions"))
}

fn workspace(config: &HttpRuntimeConfig) -> PathBuf {
    config
        .workspace
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn read_json_file(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn session_status_text(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::Paused => "paused",
        SessionStatus::Stop => "stop",
        SessionStatus::Compacting => "compacting",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}_{}", now_ms(), std::process::id())
}
