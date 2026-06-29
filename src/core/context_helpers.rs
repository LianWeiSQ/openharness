#[must_use]
pub fn estimate_text_tokens(text: &str, bytes_per_token: u64) -> u64 {
    let bytes_per_token = if bytes_per_token == 0 {
        DEFAULT_BYTES_PER_TOKEN
    } else {
        bytes_per_token
    };
    let byte_count = text.len() as u64;
    byte_count.div_ceil(bytes_per_token).max(1)
}

fn merge_compaction_facade_options(
    options: Option<&Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let raw_options = options.and_then(Value::as_object);
    let mut raw_context = match raw_options.and_then(|items| items.get("context_budget")) {
        Some(Value::Null) | None => BTreeMap::new(),
        Some(Value::Object(items)) => items
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        Some(_) => return Err("AgentConfig.options['context_budget'] must be a dict.".to_string()),
    };
    let Some(raw_compaction) = raw_options.and_then(|items| items.get("compaction")) else {
        return Ok(raw_context);
    };
    let Value::Object(compaction) = raw_compaction else {
        return Err("AgentConfig.options['compaction'] must be a dict.".to_string());
    };
    let mut merged = BTreeMap::new();
    if let Some(value) = compaction.get("auto") {
        let auto = expect_bool(value, "auto", "compaction")?;
        merged.insert(
            "strategy".to_string(),
            Value::String(if auto { "auto" } else { "error" }.to_string()),
        );
    }
    if let Some(value) = compaction.get("prune") {
        merged.insert(
            "prune_old_tool_outputs".to_string(),
            Value::Bool(expect_bool(value, "prune", "compaction")?),
        );
    }
    if let Some(value) = compaction.get("reserved") {
        merged.insert(
            "input_safety_margin_tokens".to_string(),
            json!(expect_int(value, "reserved", 0, "compaction")?),
        );
    }
    if let Some(value) = compaction.get("mode") {
        let mode = expect_non_empty_string(value, "compaction.mode")?;
        merged.insert("compaction_mode".to_string(), Value::String(mode));
    }
    merged.append(&mut raw_context);
    Ok(merged)
}

fn expect_non_empty_string(value: &Value, field_name: &str) -> Result<String, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{field_name} must be a non-empty string."))?
        .trim()
        .to_string();
    if text.is_empty() {
        return Err(format!("{field_name} must be a non-empty string."));
    }
    Ok(text)
}

fn expect_bool(value: &Value, field_name: &str, prefix: &str) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("{prefix}.{field_name} must be a bool."))
}

fn expect_int(value: &Value, field_name: &str, minimum: u64, prefix: &str) -> Result<u64, String> {
    let Some(number) = value.as_u64() else {
        return Err(format!("{prefix}.{field_name} must be an int."));
    };
    if number < minimum {
        return Err(format!("{prefix}.{field_name} must be >= {minimum}."));
    }
    Ok(number)
}

fn expect_float(
    value: &Value,
    field_name: &str,
    minimum: f64,
    maximum: f64,
    include_minimum: bool,
) -> Result<f64, String> {
    let Some(number) = value.as_f64() else {
        return Err(format!("context_budget.{field_name} must be a number."));
    };
    if include_minimum {
        if number < minimum {
            return Err(format!("context_budget.{field_name} must be >= {minimum}."));
        }
    } else if number <= minimum {
        return Err(format!("context_budget.{field_name} must be > {minimum}."));
    }
    if number > maximum {
        return Err(format!("context_budget.{field_name} must be <= {maximum}."));
    }
    Ok(number)
}

fn compute_input_limit_tokens(model: &Model, config: &ContextBudgetOptions) -> u64 {
    if config.use_safety_margin_tokens {
        let limit = model
            .context_window
            .saturating_sub(config.reserve_output_tokens)
            .saturating_sub(config.input_safety_margin_tokens);
        if limit > 0 || config.explicit_input_safety_margin_tokens {
            return limit;
        }
    }
    ((model.context_window as f64 * config.guard_ratio) as u64)
        .saturating_sub(config.reserve_output_tokens)
}

fn options_to_btree(options: Option<&Value>) -> BTreeMap<String, Value> {
    options
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(Map::iter)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn is_openai_compatible_model(model: &Model) -> bool {
    matches!(
        model.provider_id.as_str(),
        "openai" | "azure_openai" | "openai_compatible"
    )
}

fn estimate_payload_tokens(payload: &MaterializedPayload, bytes_per_token: u64) -> u64 {
    let serialized = serde_json::to_string(payload).unwrap_or_default();
    let bytes_per_token = bytes_per_token.max(1);
    (serialized.len() as u64).div_ceil(bytes_per_token).max(1)
}

struct ToolDiagnostics {
    tool_message_count: u64,
    largest_tool_message_tokens: u64,
    largest_tool_message_name: String,
}

fn tool_message_diagnostics(
    messages: &[ChatMessage],
    model: &Model,
    options: Option<&Value>,
    bytes_per_token: u64,
) -> ToolDiagnostics {
    let mut result = ToolDiagnostics {
        tool_message_count: 0,
        largest_tool_message_tokens: 0,
        largest_tool_message_name: String::new(),
    };
    for message in messages {
        if message.role != Role::Tool {
            continue;
        }
        result.tool_message_count += 1;
        let payload = materialize_openai_compatible_payload(
            None,
            std::slice::from_ref(message),
            &[],
            Some(model),
            Some(&options_to_btree(options)),
        );
        let estimate = estimate_payload_tokens(&payload, bytes_per_token);
        if estimate > result.largest_tool_message_tokens {
            result.largest_tool_message_tokens = estimate;
            result.largest_tool_message_name = message.name.clone().unwrap_or_default();
        }
    }
    result
}

fn work_state_item(
    metadata: &BTreeMap<String, Value>,
    message_count: usize,
) -> Option<ContextItem> {
    let compaction = get_context_compaction(metadata, message_count)?;
    let summary = compaction.get("summary")?.as_str()?.to_string();
    let mut item = ContextItem::new(
        "work_state:context_compaction",
        "work_state",
        "session.metadata.context_compaction",
        summary,
        95,
    );
    item.pinned = true;
    item.metadata.insert(
        "compacted_until".to_string(),
        compaction
            .get("compacted_until")
            .cloned()
            .unwrap_or(Value::Null),
    );
    item.metadata.insert(
        "format".to_string(),
        compaction.get("format").cloned().unwrap_or(Value::Null),
    );
    item.metadata.insert(
        "schema_version".to_string(),
        compaction
            .get("schema_version")
            .cloned()
            .unwrap_or(Value::Null),
    );
    item.metadata.insert(
        "source".to_string(),
        compaction.get("source").cloned().unwrap_or(Value::Null),
    );
    Some(item)
}

fn get_context_compaction(
    metadata: &BTreeMap<String, Value>,
    message_count: usize,
) -> Option<BTreeMap<String, Value>> {
    let raw = metadata.get("context_compaction")?.as_object()?;
    let compacted_until = raw.get("compacted_until")?.as_u64()?;
    if compacted_until == 0 || compacted_until as usize > message_count {
        return None;
    }
    let summary = render_compaction_summary(raw)?;
    if summary.trim().is_empty() {
        return None;
    }
    let mut result = BTreeMap::new();
    result.insert(
        "summary".to_string(),
        Value::String(summary.trim().to_string()),
    );
    result.insert("compacted_until".to_string(), json!(compacted_until));
    result.insert(
        "updated_at".to_string(),
        raw.get("updated_at")
            .and_then(Value::as_u64)
            .map_or_else(|| json!(0), |value| json!(value)),
    );
    for key in ["schema_version", "format", "state", "source", "parse_error"] {
        if let Some(value) = raw.get(key) {
            result.insert(key.to_string(), value.clone());
        }
    }
    Some(result)
}

fn render_compaction_summary(raw: &Map<String, Value>) -> Option<String> {
    if raw.get("format").and_then(Value::as_str) == Some("structured_work_state")
        && let Some(state) = raw.get("state").and_then(Value::as_object)
    {
        return Some(render_work_state(&work_state_from_map(state)));
    }
    if let Some(summary) = raw.get("summary").and_then(Value::as_str)
        && !summary.trim().is_empty()
    {
        return Some(summary.trim().to_string());
    }
    raw.get("state")
        .and_then(Value::as_object)
        .map(|state| render_work_state(&work_state_from_map(state)))
}

fn work_state_from_map(state: &Map<String, Value>) -> WorkState {
    WorkState {
        task: string_field(state, "task"),
        progress: string_vec_field(state, "progress"),
        decisions: string_vec_field(state, "decisions"),
        files: state
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .map(|item| WorkStateFile {
                path: string_field(item, "path"),
                status: string_field(item, "status"),
                note: string_field(item, "note"),
            })
            .collect(),
        tool_findings: string_vec_field(state, "tool_findings"),
        todos: string_vec_field(state, "todos"),
        open_questions: string_vec_field(state, "open_questions"),
        blockers: string_vec_field(state, "blockers"),
        next_steps: string_vec_field(state, "next_steps"),
        risks: string_vec_field(state, "risks"),
    }
}

fn sandbox_item(execution: &Value) -> Option<ContextItem> {
    let execution = execution.as_object()?;
    let mode = execution.get("mode").and_then(Value::as_str)?.trim();
    if mode.is_empty() || mode == "local" {
        return None;
    }
    let mut safe_payload = BTreeMap::new();
    for key in ["mode", "sandbox_id", "remote_workdir"] {
        if let Some(value) = execution.get(key)
            && !value.is_null()
        {
            safe_payload.insert(key.to_string(), value.clone());
        }
    }
    let mut item = ContextItem::new(
        "sandbox:execution",
        "sandbox",
        "session.metadata.execution",
        format!(
            "[Sandbox context]\n{}",
            stable_json_dumps(&json!(safe_payload))
        ),
        85,
    );
    item.pinned = true;
    item.stable_prefix = true;
    item.metadata = safe_payload;
    Some(item)
}

fn todo_item(todos: &[Value]) -> Option<ContextItem> {
    if todos.is_empty() {
        return None;
    }
    let mut normalized = Vec::new();
    for (index, todo) in todos.iter().enumerate() {
        let mut payload = todo.as_object().cloned().unwrap_or_default();
        payload
            .entry("id".to_string())
            .or_insert_with(|| json!(format!("todo-{}", index + 1)));
        normalized.push(payload);
    }
    if normalized.is_empty() {
        return None;
    }
    let mut lines = vec!["[Todos]".to_string()];
    for todo in &normalized {
        lines.push(format!(
            "- ({}/{}) {}",
            todo.get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending"),
            todo.get("priority")
                .and_then(Value::as_str)
                .unwrap_or("medium"),
            todo.get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ));
    }
    let mut item = ContextItem::new(
        "todo:session",
        "todo",
        "session.todos",
        lines.join("\n").trim().to_string(),
        80,
    );
    item.metadata
        .insert("count".to_string(), json!(normalized.len()));
    Some(item)
}

fn message_items(messages: &[ChatMessage]) -> Vec<ContextItem> {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let kind = if message.role == Role::Tool {
                "tool_result"
            } else {
                "message"
            };
            let identifier = message
                .tool_call_id
                .clone()
                .unwrap_or_else(|| format!("{}:{index}", role_str(&message.role)));
            let mut metadata = BTreeMap::new();
            metadata.insert("role".to_string(), json!(role_str(&message.role)));
            metadata.insert(
                "name".to_string(),
                message.name.clone().map_or(Value::Null, Value::String),
            );
            metadata.insert(
                "tool_call_id".to_string(),
                message
                    .tool_call_id
                    .clone()
                    .map_or(Value::Null, Value::String),
            );
            let mut item = ContextItem::new(
                format!("{kind}:{identifier}"),
                kind,
                format!("session.messages[{index}]"),
                message.content.clone(),
                if kind == "tool_result" { 50 } else { 40 },
            );
            item.metadata = metadata;
            item
        })
        .collect()
}

fn item_to_message(item: &ContextItem) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: item.content.clone(),
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::from([
            ("synthetic_context_item".to_string(), json!(true)),
            ("context_item_id".to_string(), json!(item.id)),
            ("context_item_kind".to_string(), json!(item.kind)),
            ("context_item_source".to_string(), json!(item.source)),
        ]),
    }
}

fn item_rank(item: &ContextItem) -> (u8, i64, u64) {
    (u8::from(item.pinned), item.priority, item.token_estimate)
}
