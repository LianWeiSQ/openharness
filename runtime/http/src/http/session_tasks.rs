fn session_tasks_payload(config: &HttpRuntimeConfig, session_id: &str) -> Value {
    let root = session_root(config);
    let mut all_tasks = Vec::new();
    let mut tasks = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let state = read_json_file(&path.join("state.latest.json"));
            let metadata = state
                .get("metadata")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let parent = metadata
                .get("parent_session_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let subagent = metadata
                .get("subagent")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if subagent {
                let task = session_task_summary_from_state(
                    &root,
                    &state,
                    &entry.file_name().to_string_lossy(),
                );
                if parent == session_id {
                    tasks.push(task.clone());
                }
                all_tasks.push(task);
            }
        }
    }
    tasks.sort_by(|left, right| {
        right["updated_at_ms"]
            .as_u64()
            .cmp(&left["updated_at_ms"].as_u64())
    });
    let tree = task_tree_for_parent(&all_tasks, session_id);
    let flat_tasks = flatten_task_tree(&tree);
    json!({
        "session_id": session_id,
        "tasks": tasks,
        "flat_tasks": flat_tasks,
        "tree": tree,
    })
}

fn task_tree_for_parent(all_tasks: &[Value], parent_session_id: &str) -> Vec<Value> {
    let mut visited = BTreeSet::new();
    task_tree_for_parent_inner(all_tasks, parent_session_id, &mut visited)
}

fn task_tree_for_parent_inner(
    all_tasks: &[Value],
    parent_session_id: &str,
    visited: &mut BTreeSet<String>,
) -> Vec<Value> {
    let mut children = all_tasks
        .iter()
        .filter(|task| {
            task.get("parent_session_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                == parent_session_id
        })
        .cloned()
        .collect::<Vec<_>>();
    children.sort_by(|left, right| {
        right["updated_at_ms"]
            .as_u64()
            .cmp(&left["updated_at_ms"].as_u64())
    });
    children
        .into_iter()
        .filter_map(|mut task| {
            let task_id = task
                .get("session_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if task_id.is_empty() || !visited.insert(task_id.clone()) {
                return None;
            }
            let nested = task_tree_for_parent_inner(all_tasks, &task_id, visited);
            if let Some(object) = task.as_object_mut() {
                object.insert("children".to_string(), Value::Array(nested));
            }
            Some(task)
        })
        .collect()
}

fn flatten_task_tree(tree: &[Value]) -> Vec<Value> {
    let mut flat = Vec::new();
    for task in tree {
        let mut without_children = task.clone();
        let children = without_children
            .as_object_mut()
            .and_then(|object| object.remove("children"))
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        flat.push(without_children);
        flat.extend(flatten_task_tree(&children));
    }
    flat
}

fn run_session_task_payload(
    config: &HttpRuntimeConfig,
    parent_session_id: &str,
    task_id: &str,
    body: &str,
) -> Result<Value, String> {
    if !valid_session_id(parent_session_id) || !valid_session_id(task_id) {
        return Err("invalid session id".to_string());
    }
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let store = FileSessionStore::new(session_root(config));
    let mut child_session = store
        .load_session(task_id)
        .map_err(|error| error.to_string())?;
    let parent = child_session
        .metadata
        .get("parent_session_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if parent != parent_session_id {
        return Err("task does not belong to parent session".to_string());
    }
    if !child_session
        .metadata
        .get("subagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err("session is not a subagent task".to_string());
    }
    let task_status = child_session
        .metadata
        .get("task_status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if task_status != "queued" {
        return Err(format!("task is not queued: {task_status}"));
    }
    let _task_run_lock = claim_session_task_run_lock(config, task_id)?;
    child_session = store
        .load_session(task_id)
        .map_err(|error| error.to_string())?;
    let parent = child_session
        .metadata
        .get("parent_session_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if parent != parent_session_id {
        return Err("task does not belong to parent session".to_string());
    }
    if !child_session
        .metadata
        .get("subagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err("session is not a subagent task".to_string());
    }
    let task_status = child_session
        .metadata
        .get("task_status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if task_status != "queued" {
        return Err(format!("task is not queued: {task_status}"));
    }

    let state = read_json_file(&session_root(config).join(task_id).join("state.latest.json"));
    let run_id = state
        .get("run_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| new_id("turn"));
    let agent_name = child_session
        .metadata
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or("subagent")
        .to_string();
    let provider = child_session
        .metadata
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("openai")
        .to_string();
    let model = child_session
        .metadata
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    let permission_raw = child_session
        .metadata
        .get("permission")
        .and_then(Value::as_str)
        .unwrap_or("PLAN_ONLY")
        .to_string();
    let permission_ruleset = parse_permission_ruleset(&permission_raw)?;
    let max_steps = child_session
        .metadata
        .get("max_steps")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| provider_max_steps(&payload));
    let skip_permissions = skip_permissions_for_turn(&payload);

    child_session.status = SessionStatus::Running;
    child_session
        .metadata
        .insert("task_status".to_string(), json!("running"));
    child_session.metadata.insert(
        "run_started_by".to_string(),
        json!(if payload
            .get("background_worker")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "background_worker"
        } else {
            "run_task"
        }),
    );
    child_session
        .metadata
        .insert("run_claimed_at_ms".to_string(), json!(now_ms()));
    store
        .start_run(
            &mut child_session,
            StartRunOptions {
                run_id: run_id.clone(),
                trace_id: new_id("trace"),
                agent_name,
                model_id: model.clone(),
                provider_id: Some(provider.clone()),
                permission: if skip_permissions {
                    format!("auto_allow:{permission_raw}")
                } else {
                    permission_raw.clone()
                },
                max_steps,
                started_at_ms: None,
            },
        )
        .map_err(|error| format!("failed to start task run: {error}"))?;

    for (index, message) in child_session.messages.iter().enumerate() {
        store
            .append_message(&child_session, message, &run_id, index as u64)
            .map_err(|error| format!("failed to record task prompt: {error}"))?;
    }

    let mut child_payload = provider_resume_payload(&payload);
    if let Some(object) = child_payload.as_object_mut() {
        object.insert("max_steps".to_string(), json!(max_steps));
    }
    let loop_result = run_provider_loop(RuntimeProviderLoopInput {
        store: &store,
        session: &mut child_session,
        run_id: &run_id,
        payload: &child_payload,
        permission_ruleset,
        skip_permissions,
        events: Vec::new(),
        carry: RuntimeProviderLoopCarry::default(),
    });

    let (status, output) = match loop_result {
        Ok(value) => {
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("completed")
                .to_string();
            child_session
                .metadata
                .insert("task_status".to_string(), json!(status.clone()));
            let _ = store.save_state(&child_session, Some(&run_id));
            (status, value)
        }
        Err(error) => {
            child_session.status = SessionStatus::Idle;
            child_session
                .metadata
                .insert("task_status".to_string(), json!("failed"));
            let _ = store.finish_run(
                &child_session,
                &run_id,
                "failed",
                1,
                Some("error"),
                Some(&error),
            );
            let _ = store.save_state(&child_session, Some(&run_id));
            (
                "failed".to_string(),
                json!({"status": "failed", "error": error}),
            )
        }
    };
    let state = read_json_file(&session_root(config).join(task_id).join("state.latest.json"));
    let task = session_task_summary_from_state(&session_root(config), &state, task_id);
    Ok(json!({
        "session_id": parent_session_id,
        "task_id": task_id,
        "run_id": run_id,
        "status": status,
        "task": task,
        "result": output,
    }))
}

struct RuntimeTaskRunLock {
    path: PathBuf,
}

impl Drop for RuntimeTaskRunLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn task_run_lock_path(config: &HttpRuntimeConfig, task_id: &str) -> PathBuf {
    session_root(config).join(task_id).join("task.run.lock")
}

fn claim_session_task_run_lock(
    config: &HttpRuntimeConfig,
    task_id: &str,
) -> Result<RuntimeTaskRunLock, String> {
    let path = task_run_lock_path(config, task_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    match create_session_task_run_lock(&path, task_id) {
        Ok(lock) => Ok(lock),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !remove_stale_task_run_lock(&path)? {
                return Err("task is already running".to_string());
            }
            match create_session_task_run_lock(&path, task_id) {
                Ok(lock) => Ok(lock),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    Err("task is already running".to_string())
                }
                Err(error) => Err(format!("failed to claim task run lock: {error}")),
            }
        }
        Err(error) => Err(format!("failed to claim task run lock: {error}")),
    }
}

fn create_session_task_run_lock(path: &Path, task_id: &str) -> std::io::Result<RuntimeTaskRunLock> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let payload = json!({
        "task_id": task_id,
        "claimed_at_ms": now_ms(),
    });
    if let Err(error) = writeln!(file, "{payload}") {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(RuntimeTaskRunLock {
        path: path.to_path_buf(),
    })
}

fn task_run_lock_stale_ms() -> u64 {
    std::env::var("OPENAGENT_TASK_RUN_LOCK_STALE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TASK_RUN_LOCK_STALE_MS)
}

fn remove_stale_task_run_lock(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let lock = read_json_file(path);
    let Some(claimed_at_ms) = lock.get("claimed_at_ms").and_then(Value::as_u64) else {
        return Ok(false);
    };
    if now_ms().saturating_sub(claimed_at_ms) < task_run_lock_stale_ms() {
        return Ok(false);
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!("failed to remove stale task run lock: {error}")),
    }
}

fn cancel_session_task_payload(
    config: &HttpRuntimeConfig,
    parent_session_id: &str,
    task_id: &str,
) -> Result<Value, String> {
    if !valid_session_id(parent_session_id) || !valid_session_id(task_id) {
        return Err("invalid session id".to_string());
    }
    let store = FileSessionStore::new(session_root(config));
    let mut child_session = store
        .load_session(task_id)
        .map_err(|error| error.to_string())?;
    let parent = child_session
        .metadata
        .get("parent_session_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if parent != parent_session_id {
        return Err("task does not belong to parent session".to_string());
    }
    if !child_session
        .metadata
        .get("subagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err("session is not a subagent task".to_string());
    }
    let task_status = child_session
        .metadata
        .get("task_status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if task_status != "queued" {
        return Err(format!("task is not queued: {task_status}"));
    }
    let lock_path = task_run_lock_path(config, task_id);
    if lock_path.exists() && !remove_stale_task_run_lock(&lock_path)? {
        return Err("task is already running".to_string());
    }
    let state = read_json_file(&session_root(config).join(task_id).join("state.latest.json"));
    let run_id = state
        .get("run_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| new_id("turn"));
    child_session.status = SessionStatus::Idle;
    child_session
        .metadata
        .insert("task_status".to_string(), json!("canceled"));
    child_session
        .metadata
        .insert("canceled_at_ms".to_string(), json!(now_ms()));
    store
        .save_state(&child_session, Some(&run_id))
        .map_err(|error| format!("failed to cancel task: {error}"))?;
    let state = read_json_file(&session_root(config).join(task_id).join("state.latest.json"));
    let task = session_task_summary_from_state(&session_root(config), &state, task_id);
    Ok(json!({
        "session_id": parent_session_id,
        "task_id": task_id,
        "run_id": run_id,
        "status": "canceled",
        "task": task,
    }))
}
