struct RuntimeTaskExecutionContext<'a> {
    store: &'a FileSessionStore,
    parent_session: &'a Session,
    parent_run_id: &'a str,
    payload: &'a Value,
    skip_permissions: bool,
}

fn execute_runtime_tool_call(
    toolkit: &Toolkit,
    tool_call: &ToolCall,
    ctx: &mut ToolContext,
    task_context: RuntimeTaskExecutionContext<'_>,
) -> ToolResult {
    if tool_call.name == TASK_TOOL_ID {
        execute_runtime_task_tool_call(toolkit, tool_call, ctx, task_context)
    } else {
        toolkit.execute(
            &tool_call.name,
            tool_call.input.clone(),
            &tool_call.call_id,
            ctx,
        )
    }
}

fn execute_runtime_task_tool_call(
    toolkit: &Toolkit,
    tool_call: &ToolCall,
    ctx: &mut ToolContext,
    task_context: RuntimeTaskExecutionContext<'_>,
) -> ToolResult {
    if let Some(result) =
        toolkit.permission_result_for_tool(TASK_TOOL_ID, &tool_call.input, &tool_call.call_id, ctx)
    {
        return result;
    }
    let input = &tool_call.input;
    let background = input
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let subagent_type = match runtime_task_input_string(input, "subagent_type")
        .or_else(|_| runtime_task_input_string(input, "agent_type"))
        .or_else(|_| runtime_task_input_string(input, "agent"))
    {
        Ok(value) => value,
        Err(error) => return runtime_task_tool_error(tool_call, &error, BTreeMap::new()),
    };
    let prompt = match runtime_task_input_string(input, "prompt") {
        Ok(value) => value,
        Err(error) => return runtime_task_tool_error(tool_call, &error, BTreeMap::new()),
    };
    let description =
        runtime_task_input_string(input, "description").unwrap_or_else(|_| subagent_type.clone());
    let profile =
        match runtime_subagent_profile(&subagent_type, &task_context.parent_session.directory) {
            Some(profile) => profile,
            None => {
                return runtime_task_tool_error(
                    tool_call,
                    &format!("subagent profile not found: {subagent_type}"),
                    BTreeMap::from([("subagent_type".to_string(), json!(subagent_type))]),
                );
            }
        };
    let child_permission = profile.permission.clone();
    let child_provider = profile
        .provider
        .clone()
        .or_else(|| {
            task_context
                .payload
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            task_context
                .parent_session
                .metadata
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "openai".to_string());
    let child_model = profile
        .model
        .clone()
        .or_else(|| {
            task_context
                .payload
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            task_context
                .parent_session
                .metadata
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(default_model_id);
    let child_max_steps = profile
        .max_steps
        .unwrap_or_else(|| provider_max_steps(task_context.payload));
    let task_id = input
        .get("task_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut child_session = match task_id.as_deref() {
        Some(existing) => match task_context.store.load_session(existing) {
            Ok(session) => session,
            Err(error) => {
                return runtime_task_tool_error(
                    tool_call,
                    &format!("failed to resume task session {existing}: {error}"),
                    BTreeMap::from([("task_id".to_string(), json!(existing))]),
                );
            }
        },
        None => Session::new(
            new_id("subtask"),
            task_context.parent_session.directory.clone(),
        ),
    };
    if let Some(existing) = task_id.as_deref() {
        if let Err(error) = validate_runtime_task_resume_session(
            &child_session,
            task_context.parent_session,
            &profile,
            existing,
        ) {
            return runtime_task_tool_error(
                tool_call,
                &error,
                BTreeMap::from([
                    ("subagent_type".to_string(), json!(profile.id.clone())),
                    ("task_id".to_string(), json!(existing)),
                ]),
            );
        }
    }
    if let Some(error) = runtime_task_governance_error(task_context.parent_session, &profile) {
        return runtime_task_tool_error(
            tool_call,
            &error,
            BTreeMap::from([
                ("tool".to_string(), json!(TASK_TOOL_ID)),
                ("subagent_type".to_string(), json!(profile.id.clone())),
                ("status".to_string(), json!("failed")),
                (
                    "task_depth".to_string(),
                    json!(runtime_child_task_depth(task_context.parent_session)),
                ),
                ("max_task_depth".to_string(), json!(max_subagent_depth())),
                (
                    "task_lineage_subagents".to_string(),
                    json!(runtime_parent_task_lineage(task_context.parent_session)),
                ),
            ]),
        );
    }
    let child_task_depth = runtime_child_task_depth(task_context.parent_session);
    let task_root_session_id = runtime_task_root_session_id(task_context.parent_session);
    let task_lineage_subagents =
        runtime_child_task_lineage(task_context.parent_session, &profile.id);
    let child_run_id = new_id("turn");
    child_session.status = SessionStatus::Running;
    child_session
        .metadata
        .insert("agent".to_string(), json!(profile.id.clone()));
    child_session
        .metadata
        .insert("provider".to_string(), json!(child_provider.clone()));
    child_session
        .metadata
        .insert("model".to_string(), json!(child_model.clone()));
    child_session.metadata.insert(
        "model_options".to_string(),
        json!(profile.model_options.clone()),
    );
    if let Some(temperature) = profile.temperature {
        child_session
            .metadata
            .insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = profile.top_p {
        child_session
            .metadata
            .insert("top_p".to_string(), json!(top_p));
    }
    if let Some(color) = profile.color.as_deref() {
        child_session
            .metadata
            .insert("color".to_string(), json!(color));
    }
    child_session
        .metadata
        .insert("subagent".to_string(), json!(true));
    child_session.metadata.insert(
        "parent_session_id".to_string(),
        json!(task_context.parent_session.id.clone()),
    );
    child_session.metadata.insert(
        "task_parent_session_id".to_string(),
        json!(task_context.parent_session.id.clone()),
    );
    child_session.metadata.insert(
        "task_root_session_id".to_string(),
        json!(task_root_session_id.clone()),
    );
    child_session
        .metadata
        .insert("task_depth".to_string(), json!(child_task_depth));
    child_session.metadata.insert(
        "task_lineage_subagents".to_string(),
        json!(task_lineage_subagents.clone()),
    );
    child_session.metadata.insert(
        "parent_run_id".to_string(),
        json!(task_context.parent_run_id),
    );
    child_session.metadata.insert(
        "parent_tool_call_id".to_string(),
        json!(tool_call.call_id.clone()),
    );
    child_session
        .metadata
        .insert("task_description".to_string(), json!(description.clone()));
    child_session
        .metadata
        .insert("task_subagent_type".to_string(), json!(profile.id.clone()));
    child_session
        .metadata
        .insert("permission".to_string(), json!(child_permission.as_str()));
    child_session
        .metadata
        .insert("max_steps".to_string(), json!(child_max_steps));
    if task_id.is_some() {
        let resume_count = child_session
            .metadata
            .get("task_resume_count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            .saturating_add(1);
        child_session
            .metadata
            .insert("task_resume_count".to_string(), json!(resume_count));
        child_session
            .metadata
            .insert("task_resumed_at_ms".to_string(), json!(now_ms()));
    }
    if background {
        child_session
            .metadata
            .insert("task_status".to_string(), json!("queued"));
        child_session
            .metadata
            .insert("background".to_string(), json!(true));
    } else {
        child_session
            .metadata
            .insert("background".to_string(), json!(false));
    }
    child_session.metadata.insert(
        "agent_profile".to_string(),
        runtime_subagent_public_value(&profile),
    );
    let system_message = bind_runtime_subagent_system_prompt(&mut child_session, &profile);
    let user = runtime_chat_message(Role::User, prompt.clone());
    let user_index = child_session.messages.len() as u64;
    child_session.add(user.clone());

    if background {
        child_session.status = SessionStatus::Idle;
        if let Err(error) = task_context
            .store
            .save_state(&child_session, Some(&child_run_id))
        {
            return runtime_task_tool_error(
                tool_call,
                &format!("failed to queue background subagent session: {error}"),
                BTreeMap::from([("subagent_type".to_string(), json!(profile.id.clone()))]),
            );
        }
        return ToolResult {
            call_id: tool_call.call_id.clone(),
            output: render_runtime_task_output(
                &child_session.id,
                "queued",
                "Background subagent task queued.",
            ),
            error: None,
            metadata: BTreeMap::from([
                ("tool".to_string(), json!(TASK_TOOL_ID)),
                ("title".to_string(), json!(description)),
                ("subagent_type".to_string(), json!(profile.id.clone())),
                ("task_id".to_string(), json!(child_session.id.clone())),
                ("session_id".to_string(), json!(child_session.id.clone())),
                ("run_id".to_string(), json!(child_run_id)),
                ("status".to_string(), json!("queued")),
                ("background".to_string(), json!(true)),
                ("provider".to_string(), json!(child_provider)),
                ("model".to_string(), json!(child_model)),
                (
                    "model_options".to_string(),
                    json!(profile.model_options.clone()),
                ),
                ("max_steps".to_string(), json!(child_max_steps)),
                ("task_depth".to_string(), json!(child_task_depth)),
                (
                    "task_root_session_id".to_string(),
                    json!(task_root_session_id),
                ),
                (
                    "task_parent_session_id".to_string(),
                    json!(task_context.parent_session.id.clone()),
                ),
                (
                    "task_lineage_subagents".to_string(),
                    json!(task_lineage_subagents),
                ),
                (
                    "agent_profile".to_string(),
                    runtime_subagent_public_value(&profile),
                ),
            ]),
        };
    }

    if let Err(error) = task_context.store.start_run(
        &mut child_session,
        StartRunOptions {
            run_id: child_run_id.clone(),
            trace_id: new_id("trace"),
            agent_name: profile.id.clone(),
            model_id: Some(child_model.clone()),
            provider_id: Some(child_provider.clone()),
            permission: if task_context.skip_permissions {
                format!("auto_allow:{}", child_permission.as_str())
            } else {
                child_permission.as_str().to_string()
            },
            max_steps: child_max_steps,
            started_at_ms: None,
        },
    ) {
        return runtime_task_tool_error(
            tool_call,
            &format!("failed to start subagent session: {error}"),
            BTreeMap::from([("subagent_type".to_string(), json!(profile.id.clone()))]),
        );
    }
    if let Some((system, system_index)) = system_message {
        let _ =
            task_context
                .store
                .append_message(&child_session, &system, &child_run_id, system_index);
    }
    let _ = task_context
        .store
        .append_message(&child_session, &user, &child_run_id, user_index);

    let mut child_payload = provider_resume_payload(task_context.payload);
    if let Some(object) = child_payload.as_object_mut() {
        object.insert("max_steps".to_string(), json!(child_max_steps));
    }
    let child_result = run_provider_loop(RuntimeProviderLoopInput {
        store: task_context.store,
        session: &mut child_session,
        run_id: &child_run_id,
        payload: &child_payload,
        permission_ruleset: child_permission,
        skip_permissions: task_context.skip_permissions,
        events: Vec::new(),
        carry: RuntimeProviderLoopCarry::default(),
    });
    match child_result {
        Ok(value) => {
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("completed");
            let final_answer = value
                .get("turn")
                .and_then(|turn| turn.get("final_answer"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if status != "completed" {
                return runtime_task_tool_error(
                    tool_call,
                    &format!("subagent {} finished with status {status}", profile.id),
                    BTreeMap::from([
                        ("tool".to_string(), json!(TASK_TOOL_ID)),
                        ("title".to_string(), json!(description)),
                        ("subagent_type".to_string(), json!(profile.id.clone())),
                        ("task_id".to_string(), json!(child_session.id.clone())),
                        ("session_id".to_string(), json!(child_session.id.clone())),
                        ("run_id".to_string(), json!(child_run_id)),
                        ("status".to_string(), json!(status)),
                        ("provider".to_string(), json!(child_provider)),
                        ("model".to_string(), json!(child_model)),
                        (
                            "model_options".to_string(),
                            json!(profile.model_options.clone()),
                        ),
                        ("max_steps".to_string(), json!(child_max_steps)),
                        ("task_depth".to_string(), json!(child_task_depth)),
                        (
                            "task_root_session_id".to_string(),
                            json!(task_root_session_id.clone()),
                        ),
                        (
                            "task_parent_session_id".to_string(),
                            json!(task_context.parent_session.id.clone()),
                        ),
                        (
                            "task_lineage_subagents".to_string(),
                            json!(task_lineage_subagents.clone()),
                        ),
                        (
                            "agent_profile".to_string(),
                            runtime_subagent_public_value(&profile),
                        ),
                    ]),
                );
            }
            ToolResult {
                call_id: tool_call.call_id.clone(),
                output: render_runtime_task_output(&child_session.id, "completed", &final_answer),
                error: None,
                metadata: BTreeMap::from([
                    ("tool".to_string(), json!(TASK_TOOL_ID)),
                    ("title".to_string(), json!(description)),
                    ("subagent_type".to_string(), json!(profile.id.clone())),
                    ("task_id".to_string(), json!(child_session.id.clone())),
                    ("session_id".to_string(), json!(child_session.id.clone())),
                    ("run_id".to_string(), json!(child_run_id)),
                    ("status".to_string(), json!("completed")),
                    ("provider".to_string(), json!(child_provider)),
                    ("model".to_string(), json!(child_model)),
                    (
                        "model_options".to_string(),
                        json!(profile.model_options.clone()),
                    ),
                    ("max_steps".to_string(), json!(child_max_steps)),
                    ("task_depth".to_string(), json!(child_task_depth)),
                    (
                        "task_root_session_id".to_string(),
                        json!(task_root_session_id.clone()),
                    ),
                    (
                        "task_parent_session_id".to_string(),
                        json!(task_context.parent_session.id.clone()),
                    ),
                    (
                        "task_lineage_subagents".to_string(),
                        json!(task_lineage_subagents.clone()),
                    ),
                    (
                        "agent_profile".to_string(),
                        runtime_subagent_public_value(&profile),
                    ),
                ]),
            }
        }
        Err(error) => runtime_task_tool_error(
            tool_call,
            &format!("subagent {} failed: {error}", profile.id),
            BTreeMap::from([
                ("tool".to_string(), json!(TASK_TOOL_ID)),
                ("title".to_string(), json!(description)),
                ("subagent_type".to_string(), json!(profile.id.clone())),
                ("task_id".to_string(), json!(child_session.id.clone())),
                ("session_id".to_string(), json!(child_session.id.clone())),
                ("run_id".to_string(), json!(child_run_id)),
                ("status".to_string(), json!("failed")),
                (
                    "model_options".to_string(),
                    json!(profile.model_options.clone()),
                ),
                ("task_depth".to_string(), json!(child_task_depth)),
                (
                    "task_root_session_id".to_string(),
                    json!(task_root_session_id),
                ),
                (
                    "task_parent_session_id".to_string(),
                    json!(task_context.parent_session.id.clone()),
                ),
                (
                    "task_lineage_subagents".to_string(),
                    json!(task_lineage_subagents),
                ),
            ]),
        ),
    }
}

fn validate_runtime_task_resume_session(
    child_session: &Session,
    parent_session: &Session,
    profile: &RuntimeSubagentProfile,
    task_id: &str,
) -> Result<(), String> {
    if !child_session
        .metadata
        .get("subagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(format!("task session {task_id} is not a subagent task"));
    }
    let parent_id = child_session
        .metadata
        .get("parent_session_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if parent_id != parent_session.id {
        return Err("task does not belong to parent session".to_string());
    }
    let stored_agent = child_session
        .metadata
        .get("agent")
        .and_then(Value::as_str)
        .or_else(|| {
            child_session
                .metadata
                .get("task_subagent_type")
                .and_then(Value::as_str)
        })
        .unwrap_or_default();
    if !stored_agent.is_empty() && stored_agent != profile.id {
        return Err(format!(
            "task session {task_id} belongs to subagent {stored_agent}, not {}",
            profile.id
        ));
    }
    match child_session
        .metadata
        .get("task_status")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "queued" | "running" | "canceled" => {
            return Err(format!(
                "task session {task_id} cannot be resumed while task status is {}",
                child_session
                    .metadata
                    .get("task_status")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ));
        }
        _ => {}
    }
    if matches!(
        child_session.status,
        SessionStatus::Running | SessionStatus::Paused | SessionStatus::Compacting
    ) {
        return Err(format!(
            "task session {task_id} cannot be resumed while session status is {}",
            session_status_text(&child_session.status)
        ));
    }
    Ok(())
}

fn bind_runtime_subagent_system_prompt(
    session: &mut Session,
    profile: &RuntimeSubagentProfile,
) -> Option<(ChatMessage, u64)> {
    let prompt = profile.prompt.trim_start_matches('\u{feff}').trim();
    if prompt.is_empty() {
        return None;
    }
    let already_bound = session.messages.iter().any(|message| {
        message.role == Role::System
            && message
                .metadata
                .get("agent_profile")
                .and_then(Value::as_str)
                == Some(profile.id.as_str())
    });
    if already_bound {
        return None;
    }
    let mut system = runtime_chat_message(Role::System, prompt.to_string());
    system
        .metadata
        .insert("agent_profile".to_string(), json!(profile.id.clone()));
    system
        .metadata
        .insert("agent_mode".to_string(), json!("subagent"));
    let system_index = session.messages.len() as u64;
    session.add(system.clone());
    Some((system, system_index))
}

fn runtime_subagent_public_value(profile: &RuntimeSubagentProfile) -> Value {
    json!({
        "id": profile.id.clone(),
        "name": profile.name.clone(),
        "description": profile.description.clone(),
        "mode": profile.mode.clone(),
        "permission": profile.permission.as_str(),
        "task_permissions": profile.task_permissions.clone(),
        "tools": profile.tools.clone(),
        "provider": profile.provider.clone(),
        "model": profile.model.clone(),
        "max_steps": profile.max_steps,
        "steps": profile.max_steps,
        "temperature": profile.temperature,
        "top_p": profile.top_p,
        "color": profile.color.clone(),
        "disabled": profile.disabled,
        "model_options": profile.model_options.clone(),
        "hidden": profile.hidden,
        "source_path": profile.source_path.as_ref().map(|path| path.to_string_lossy().to_string()),
    })
}

fn runtime_task_input_string(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("task tool requires non-empty {key}"))
}

fn runtime_task_tool_error(
    tool_call: &ToolCall,
    error: &str,
    mut metadata: BTreeMap<String, Value>,
) -> ToolResult {
    metadata
        .entry("tool".to_string())
        .or_insert_with(|| json!(TASK_TOOL_ID));
    ToolResult {
        call_id: tool_call.call_id.clone(),
        output: String::new(),
        error: Some(error.to_string()),
        metadata,
    }
}

fn render_runtime_task_output(task_id: &str, state: &str, text: &str) -> String {
    format!(
        "<task id=\"{}\" state=\"{}\">\n<task_result>\n{}\n</task_result>\n</task>",
        escape_runtime_task_text(task_id),
        escape_runtime_task_text(state),
        escape_runtime_task_text(text),
    )
}

fn escape_runtime_task_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
