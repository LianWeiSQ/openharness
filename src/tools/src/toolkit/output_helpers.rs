fn clip_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut clipped = text.chars().take(max_chars).collect::<String>();
    clipped.push_str("...");
    clipped
}

fn code_search_output(
    root: &Path,
    base: &Path,
    hits: Vec<String>,
    preview_hits: Vec<String>,
    truncated: bool,
) -> ToolResultValue<ToolOutput> {
    let mut output = ToolOutput::new(display_path(root, base), hits.join("\n"));
    output
        .metadata
        .insert("count".to_string(), json!(hits.len()));
    output
        .metadata
        .insert("returned_count".to_string(), json!(hits.len()));
    output
        .metadata
        .insert("preview".to_string(), json!(preview_hits.join("\n")));
    output.truncated = truncated;
    Ok(output)
}

fn save_todos(ctx: &mut ToolContext, todos: Vec<TodoItem>) -> ToolResultValue<()> {
    ctx.todos = todos.clone();
    let path = todo_storage_path(ctx);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let encoded = serde_json::to_string_pretty(&todos).map_err(|error| error.to_string())?;
    fs::write(path, encoded).map_err(io_error)
}

fn load_todos(ctx: &mut ToolContext) -> ToolResultValue<Vec<TodoItem>> {
    if !ctx.todos.is_empty() {
        return Ok(ctx.todos.clone());
    }
    let path = todo_storage_path(ctx);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path).map_err(io_error)?;
    let todos =
        serde_json::from_str::<Vec<TodoItem>>(&content).map_err(|error| error.to_string())?;
    ctx.todos = todos.clone();
    Ok(todos)
}

fn todo_storage_path(ctx: &ToolContext) -> PathBuf {
    let session_key = if ctx.session_id.is_empty() {
        "default"
    } else {
        &ctx.session_id
    };
    let safe_key = session_key
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    ctx.session_root
        .join(".openagent")
        .join("todo")
        .join(format!("{safe_key}.json"))
}

fn todo_from_value(value: &Value) -> ToolResultValue<TodoItem> {
    Ok(TodoItem::new(
        value_string(value, "content")?,
        value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending"),
        value
            .get("priority")
            .and_then(Value::as_str)
            .unwrap_or("medium"),
        value.get("id").and_then(Value::as_str).unwrap_or_default(),
    ))
}

fn todo_output(todos: Vec<TodoItem>) -> ToolResultValue<ToolOutput> {
    let open_count = todos
        .iter()
        .filter(|todo| todo.status != "completed")
        .count();
    let output_text = serde_json::to_string_pretty(&todos).map_err(|error| error.to_string())?;
    let mut output = ToolOutput::new(format!("{open_count} todos"), output_text);
    output.metadata.insert("todos".to_string(), json!(todos));
    Ok(output)
}

fn question_metadata_value(value: &Value) -> ToolResultValue<Value> {
    let options = value
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| "Expected list input for options".to_string())?;
    let option_values = options
        .iter()
        .map(|option| {
            Ok(json!({
                "label": value_string(option, "label")?,
                "description": value_string(option, "description")?,
            }))
        })
        .collect::<ToolResultValue<Vec<_>>>()?;
    Ok(json!({
        "header": value_string(value, "header")?,
        "question": value_string(value, "question")?,
        "multiple": value.get("multiple").and_then(Value::as_bool).unwrap_or(false),
        "options": option_values,
    }))
}

fn skill_roots(ctx: &ToolContext) -> Option<Vec<String>> {
    let roots = ctx
        .agent_options
        .get("skill_roots")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    (!roots.is_empty()).then_some(roots)
}

fn report_metadata(
    report: &SkillDiscoveryReport,
    query: Option<String>,
) -> BTreeMap<String, Value> {
    let mut payload = BTreeMap::from([
        ("skill_count".to_string(), json!(report.skills.len())),
        ("loaded_count".to_string(), json!(report.loaded_count)),
        ("scanned_files".to_string(), json!(report.scanned_files)),
        ("invalid_count".to_string(), json!(report.invalid_count)),
        ("duplicate_count".to_string(), json!(report.duplicate_count)),
    ]);
    if let Some(query) = query {
        payload.insert("query".to_string(), json!(query));
    }
    if !report.issues.is_empty() {
        payload.insert(
            "issues".to_string(),
            Value::Array(
                report
                    .issues
                    .iter()
                    .map(|issue| {
                        json!({
                            "kind": issue.kind,
                            "path": issue.path,
                            "message": issue.message,
                            "duplicate_of": issue.duplicate_of,
                        })
                    })
                    .collect(),
            ),
        );
    }
    payload
}

fn diagnostic_lines(report: &SkillDiscoveryReport) -> Vec<String> {
    if report.issues.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![String::new(), "Diagnostics:".to_string()];
    for issue in &report.issues {
        let suffix = issue
            .duplicate_of
            .as_ref()
            .map(|path| format!(" duplicate_of={path}"))
            .unwrap_or_default();
        lines.push(format!(
            "- {}: {} - {}{}",
            issue.kind, issue.path, issue.message, suffix
        ));
    }
    lines
}

fn title_for_questions(count: usize) -> String {
    if count == 1 {
        "Asked 1 question".to_string()
    } else {
        format!("Asked {count} questions")
    }
}

fn string_arg(input: &Value, key: &str) -> ToolResultValue<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("Missing required parameter: {key}"))
}

fn string_arg_or(input: &Value, key: &str, default: &str) -> ToolResultValue<String> {
    input
        .get(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("Expected string input for {key}"))
        })
        .unwrap_or_else(|| Ok(default.to_string()))
}

fn optional_string_arg(input: &Value, key: &str) -> ToolResultValue<Option<String>> {
    input
        .get(key)
        .map(|value| {
            if value.is_null() {
                Ok(None)
            } else {
                value
                    .as_str()
                    .map(|text| Some(text.to_string()))
                    .ok_or_else(|| format!("Expected string input for {key}"))
            }
        })
        .unwrap_or(Ok(None))
}

fn value_string(input: &Value, key: &str) -> ToolResultValue<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("Missing required parameter: {key}"))
}

fn bool_arg(input: &Value, key: &str, default: bool) -> ToolResultValue<bool> {
    input
        .get(key)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("Expected boolean input for {key}"))
        })
        .unwrap_or(Ok(default))
}

fn usize_arg(input: &Value, key: &str, default: usize) -> ToolResultValue<usize> {
    input
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .and_then(|item| usize::try_from(item).ok())
                .ok_or_else(|| format!("Expected non-negative integer input for {key}"))
        })
        .unwrap_or(Ok(default))
}

fn optional_usize_arg(input: &Value, key: &str) -> ToolResultValue<Option<usize>> {
    input
        .get(key)
        .map(|value| {
            if value.is_null() {
                Ok(None)
            } else {
                value
                    .as_u64()
                    .and_then(|item| usize::try_from(item).ok())
                    .map(Some)
                    .ok_or_else(|| format!("Expected non-negative integer input for {key}"))
            }
        })
        .unwrap_or(Ok(None))
}

fn u64_arg(input: &Value, key: &str, default: u64) -> ToolResultValue<u64> {
    input
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| format!("Expected non-negative integer input for {key}"))
        })
        .unwrap_or(Ok(default))
}

fn string_list_arg(input: &Value, key: &str) -> ToolResultValue<Vec<String>> {
    let Some(value) = input.get(key) else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let items = value
        .as_array()
        .ok_or_else(|| format!("Expected list input for {key}"))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("Expected string item in {key}"))
        })
        .collect()
}

fn write_truncated_output(root: &Path, call_id: &str, content: &str) -> ToolResultValue<PathBuf> {
    let output_dir = root.join(".openagent").join("tool_output");
    fs::create_dir_all(&output_dir).map_err(io_error)?;
    let output_path = output_dir.join(format!("{call_id}.txt"));
    fs::write(&output_path, content).map_err(io_error)?;
    Ok(output_path)
}

#[cfg(unix)]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.arg("-c").arg(command);
    shell
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("cmd");
    shell.arg("/C").arg(command);
    shell
}
