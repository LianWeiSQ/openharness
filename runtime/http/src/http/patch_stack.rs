#[derive(Clone, Debug)]
struct FileChangeBefore {
    target: PathBuf,
    display_path: String,
    existed_before: bool,
    before_content: Option<String>,
}

#[derive(Clone, Copy, Debug)]
enum FileChangeState {
    Before,
    After,
}

fn capture_file_change_before(session: &Session, call: &ToolCall) -> Option<FileChangeBefore> {
    if !matches!(call.name.as_str(), "write" | "edit") {
        return None;
    }
    let raw_path = call.input.get("file_path").and_then(Value::as_str)?;
    let target = resolve_path_in_root(&session.directory, raw_path).ok()?;
    let existed_before = target.exists();
    let before_content = if target.is_file() {
        fs::read_to_string(&target).ok()
    } else {
        None
    };
    Some(FileChangeBefore {
        display_path: session_display_path(session, &target),
        target,
        existed_before,
        before_content,
    })
}

fn file_change_preview(before: &FileChangeBefore, call: &ToolCall) -> Option<Value> {
    let after = predicted_after_content(before, call)?;
    let existed_after = true;
    let diff = render_unified_diff(
        &before.display_path,
        before.before_content.as_deref(),
        Some(after.as_str()),
    );
    Some(json!({
        "kind": "file",
        "path": before.display_path,
        "status": file_change_status(before.existed_before, existed_after),
        "diff": diff,
        "summary": format!(
            "{} {}",
            call.name,
            if before.existed_before { "will modify file" } else { "will create file" }
        ),
    }))
}

fn predicted_after_content(before: &FileChangeBefore, call: &ToolCall) -> Option<String> {
    match call.name.as_str() {
        "write" => call
            .input
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string),
        "edit" => {
            let old = call.input.get("old_string").and_then(Value::as_str)?;
            let new = call.input.get("new_string").and_then(Value::as_str)?;
            let replace_all = call
                .input
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if old.is_empty() {
                return Some(new.to_string());
            }
            preview_replace_text(before.before_content.as_deref()?, old, new, replace_all).ok()
        }
        _ => None,
    }
}

fn complete_file_change(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    call: &ToolCall,
    before: Option<FileChangeBefore>,
    result: &ToolResult,
) -> Option<Value> {
    if result.error.is_some() {
        return None;
    }
    let before = before?;
    let existed_after = before.target.exists();
    let after_content = if before.target.is_file() {
        fs::read_to_string(&before.target).ok()
    } else {
        None
    };
    if before.existed_before == existed_after && before.before_content == after_content {
        return None;
    }
    let diff = render_unified_diff(
        &before.display_path,
        before.before_content.as_deref(),
        after_content.as_deref(),
    );
    let change = json!({
        "id": new_id("patch"),
        "session_id": session.id,
        "run_id": run_id,
        "call_id": call.call_id,
        "tool": call.name,
        "created_at_ms": now_ms(),
        "workspace": session.directory.to_string_lossy(),
        "path": before.display_path,
        "absolute_path": before.target.to_string_lossy(),
        "existed_before": before.existed_before,
        "existed_after": existed_after,
        "before": before.before_content,
        "after": after_content,
        "status": "applied",
        "diff": diff,
    });
    push_file_change(session, change.clone());
    let public = public_file_change(&change);
    let _ = store.record_event(
        &session.id,
        run_id,
        "patch.detected",
        SessionEventOptions {
            kind: "patch".to_string(),
            attributes: BTreeMap::from([("patch".to_string(), public)]),
            ..SessionEventOptions::default()
        },
    );
    Some(change)
}

fn patch_detected_event(session: &Session, run_id: &str, change: &Value) -> Value {
    json!({
        "method": "patch/detected",
        "params": {
            "session_id": session.id,
            "thread_id": session.id,
            "turn_id": run_id,
            "patch": public_file_change(change),
        }
    })
}

fn append_patch_stack_event(
    store: &FileSessionStore,
    session: &Session,
    turn_id: &str,
    method: &str,
    patch: &Value,
) -> Value {
    let event_name = match method {
        "patch/undone" => "patch.undone",
        "patch/redone" => "patch.redone",
        _ => "patch.changed",
    };
    let event = json!({
        "method": method,
        "params": {
            "session_id": session.id,
            "thread_id": session.id,
            "turn_id": turn_id,
            "patch": patch,
        }
    });
    append_app_events(
        &store.root,
        &session.id,
        turn_id,
        std::slice::from_ref(&event),
    );
    let _ = store.record_event(
        &session.id,
        turn_id,
        event_name,
        SessionEventOptions {
            kind: "patch".to_string(),
            attributes: BTreeMap::from([("patch".to_string(), patch.clone())]),
            ..SessionEventOptions::default()
        },
    );
    event
}

fn push_file_change(session: &mut Session, change: Value) {
    let public = public_file_change(&change);
    let mut undo_stack = file_change_stack(session, FILE_CHANGE_UNDO_STACK_KEY);
    push_stack_entry(&mut undo_stack, change);
    set_file_change_stack(session, FILE_CHANGE_UNDO_STACK_KEY, undo_stack);
    set_file_change_stack(session, FILE_CHANGE_REDO_STACK_KEY, Vec::new());
    session
        .metadata
        .insert(FILE_CHANGE_LATEST_KEY.to_string(), public);
}

fn file_change_stack(session: &Session, key: &str) -> Vec<Value> {
    session
        .metadata
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn set_file_change_stack(session: &mut Session, key: &str, stack: Vec<Value>) {
    session
        .metadata
        .insert(key.to_string(), Value::Array(stack));
}

fn push_stack_entry(stack: &mut Vec<Value>, value: Value) {
    stack.push(value);
    let excess = stack.len().saturating_sub(MAX_FILE_CHANGE_STACK);
    if excess > 0 {
        stack.drain(0..excess);
    }
}

fn apply_file_change_state(
    session: &Session,
    change: &Value,
    state: FileChangeState,
) -> Result<(), String> {
    let path = change
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| change.get("absolute_path").and_then(Value::as_str))
        .ok_or_else(|| "patch is missing path".to_string())?;
    let target = resolve_path_in_root(&session.directory, path)?;
    let (exists_key, content_key) = match state {
        FileChangeState::Before => ("existed_before", "before"),
        FileChangeState::After => ("existed_after", "after"),
    };
    let should_exist = change
        .get(exists_key)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if should_exist {
        let content = change
            .get(content_key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("patch is missing {content_key} content"))?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(&target, content).map_err(|error| error.to_string())
    } else if target.exists() {
        if target.is_dir() {
            return Err(format!(
                "refusing to remove directory: {}",
                target.display()
            ));
        }
        fs::remove_file(&target).map_err(|error| error.to_string())
    } else {
        Ok(())
    }
}

fn mark_file_change(mut change: Value, status: &str) -> Value {
    if let Some(object) = change.as_object_mut() {
        object.insert("status".to_string(), json!(status));
        object.insert(format!("{status}_at_ms"), json!(now_ms()));
    }
    change
}

fn public_file_change(change: &Value) -> Value {
    let mut public = change.clone();
    if let Some(object) = public.as_object_mut() {
        object.remove("before");
        object.remove("after");
        object.remove("absolute_path");
    }
    public
}

fn file_change_run_id(change: &Value) -> String {
    change
        .get("run_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| new_id("turn"))
}

fn session_display_path(session: &Session, target: &Path) -> String {
    let root = session
        .directory
        .canonicalize()
        .unwrap_or_else(|_| session.directory.clone());
    target
        .strip_prefix(&root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/")
}

fn file_change_status(existed_before: bool, existed_after: bool) -> &'static str {
    match (existed_before, existed_after) {
        (false, true) => "created",
        (true, false) => "deleted",
        (true, true) => "modified",
        (false, false) => "unchanged",
    }
}

fn preview_replace_text(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, String> {
    if old == new {
        return Err("old_string and new_string must be different".to_string());
    }
    if old.is_empty() {
        return Ok(new.to_string());
    }
    let count = content.matches(old).count();
    if count == 0 {
        return Err("old_string not found in content".to_string());
    }
    if count > 1 && !replace_all {
        return Err("old_string found multiple times".to_string());
    }
    if replace_all {
        Ok(content.replace(old, new))
    } else {
        Ok(content.replacen(old, new, 1))
    }
}

fn render_unified_diff(path: &str, before: Option<&str>, after: Option<&str>) -> String {
    let before_lines = before
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let after_lines = after
        .map(|value| value.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut lines = vec![
        format!("--- a/{path}"),
        format!("+++ b/{path}"),
        "@@".to_string(),
    ];
    let diff_lines = if before_lines.len().saturating_mul(after_lines.len()) <= 200_000 {
        lcs_diff_lines(&before_lines, &after_lines)
    } else {
        full_file_diff_lines(&before_lines, &after_lines)
    };
    lines.extend(diff_lines);
    truncate_diff_lines(lines).join("\n")
}

fn lcs_diff_lines(before: &[&str], after: &[&str]) -> Vec<String> {
    let rows = before.len() + 1;
    let cols = after.len() + 1;
    let mut table = vec![0_usize; rows * cols];
    for row in 1..rows {
        for col in 1..cols {
            table[row * cols + col] = if before[row - 1] == after[col - 1] {
                table[(row - 1) * cols + col - 1] + 1
            } else {
                table[(row - 1) * cols + col].max(table[row * cols + col - 1])
            };
        }
    }
    let mut row = before.len();
    let mut col = after.len();
    let mut output = Vec::new();
    while row > 0 || col > 0 {
        if row > 0 && col > 0 && before[row - 1] == after[col - 1] {
            output.push(format!(" {}", before[row - 1]));
            row -= 1;
            col -= 1;
        } else if col > 0
            && (row == 0 || table[row * cols + col - 1] >= table[(row - 1) * cols + col])
        {
            output.push(format!("+{}", after[col - 1]));
            col -= 1;
        } else if row > 0 {
            output.push(format!("-{}", before[row - 1]));
            row -= 1;
        }
    }
    output.reverse();
    output
}

fn full_file_diff_lines(before: &[&str], after: &[&str]) -> Vec<String> {
    before
        .iter()
        .map(|line| format!("-{line}"))
        .chain(after.iter().map(|line| format!("+{line}")))
        .collect()
}

fn truncate_diff_lines(mut lines: Vec<String>) -> Vec<String> {
    if lines.len() <= MAX_RENDERED_DIFF_LINES {
        return lines;
    }
    let omitted = lines.len() - MAX_RENDERED_DIFF_LINES;
    lines.truncate(MAX_RENDERED_DIFF_LINES);
    lines.push(format!("... diff truncated ({omitted} more lines) ..."));
    lines
}

fn query_param(path: &str, target: &str) -> Option<String> {
    path.split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(key, value)| (key == target).then(|| percent_decode(value)))
}

fn percent_decode(value: &str) -> String {
    let mut bytes = Vec::new();
    let raw = value.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' && index + 2 < raw.len() {
            if let (Some(high), Some(low)) = (hex_value(raw[index + 1]), hex_value(raw[index + 2]))
            {
                bytes.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        bytes.push(if raw[index] == b'+' { b' ' } else { raw[index] });
        index += 1;
    }
    String::from_utf8_lossy(&bytes).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
