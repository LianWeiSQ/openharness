fn start_background_task_worker(config: HttpRuntimeConfig) {
    if !background_task_worker_enabled() {
        return;
    }
    let _ = thread::spawn(move || {
        loop {
            if let Err(error) = run_background_task_worker_once(&config) {
                eprintln!("openagent background task worker failed: {error}");
            }
            thread::sleep(Duration::from_millis(background_task_worker_poll_ms()));
        }
    });
}

fn background_task_worker_enabled() -> bool {
    std::env::var("OPENAGENT_BACKGROUND_WORKER")
        .ok()
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(true)
}

fn background_task_worker_poll_ms() -> u64 {
    std::env::var("OPENAGENT_BACKGROUND_WORKER_POLL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_BACKGROUND_TASK_WORKER_POLL_MS)
        .max(10)
}

fn run_background_task_worker_once(config: &HttpRuntimeConfig) -> Result<(), String> {
    for task in queued_background_task_ids(config)? {
        let payload = json!({"background_worker": true});
        match run_session_task_payload(
            config,
            &task.parent_session_id,
            &task.task_id,
            &payload.to_string(),
        ) {
            Ok(_) => {}
            Err(error)
                if error.contains("task is already running")
                    || error.contains("task is not queued") => {}
            Err(error) => eprintln!(
                "openagent background task worker could not run task {}: {}",
                task.task_id, error
            ),
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct QueuedBackgroundTask {
    parent_session_id: String,
    task_id: String,
    updated_at_ms: u64,
}

fn queued_background_task_ids(
    config: &HttpRuntimeConfig,
) -> Result<Vec<QueuedBackgroundTask>, String> {
    let root = session_root(config);
    let mut tasks = Vec::new();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(tasks),
        Err(error) => return Err(format!("failed to read session root: {error}")),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let task_id = entry.file_name().to_string_lossy().to_string();
        let state = read_json_file(&path.join("state.latest.json"));
        let metadata = state
            .get("metadata")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let is_queued_background_subagent = metadata
            .get("subagent")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && metadata
                .get("background")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            && metadata
                .get("task_status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                == "queued";
        if !is_queued_background_subagent {
            continue;
        }
        let Some(parent_session_id) = metadata
            .get("parent_session_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        tasks.push(QueuedBackgroundTask {
            parent_session_id,
            task_id,
            updated_at_ms: state
                .get("updated_at_ms")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        });
    }
    tasks.sort_by(|left, right| {
        left.updated_at_ms
            .cmp(&right.updated_at_ms)
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
    Ok(tasks)
}
