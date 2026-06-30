fn execute_task_tool_call(
    toolkit: &Toolkit,
    tool_call: &ToolCall,
    ctx: &mut ToolContext,
    task_context: TaskExecutionContext<'_>,
) -> ToolResult {
    if let Some(result) =
        toolkit.permission_result_for_tool(TASK_TOOL_ID, &tool_call.input, &tool_call.call_id, ctx)
    {
        return result;
    }
    let input = &tool_call.input;
    if input
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return task_tool_error(
            tool_call,
            "background subagent tasks are not implemented yet; omit background or set it to false",
            BTreeMap::new(),
        );
    }
    let subagent_type = match task_input_string(input, "subagent_type")
        .or_else(|_| task_input_string(input, "agent_type"))
        .or_else(|_| task_input_string(input, "agent"))
    {
        Ok(value) => value,
        Err(error) => return task_tool_error(tool_call, &error, BTreeMap::new()),
    };
    let prompt = match task_input_string(input, "prompt") {
        Ok(value) => value,
        Err(error) => return task_tool_error(tool_call, &error, BTreeMap::new()),
    };
    let description =
        task_input_string(input, "description").unwrap_or_else(|_| subagent_type.clone());
    let task_id = input
        .get("task_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let command = input
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let child_profile = match load_agent_profile_by_name(task_context.args, &subagent_type) {
        Ok(profile) => profile,
        Err(error) => {
            return task_tool_error(
                tool_call,
                &error,
                BTreeMap::from([("subagent_type".to_string(), json!(subagent_type))]),
            );
        }
    };
    if !is_subagent_mode(&child_profile.mode) {
        return task_tool_error(
            tool_call,
            &format!(
                "agent profile {} has mode {}; task can only launch subagent or all profiles",
                child_profile.id, child_profile.mode
            ),
            BTreeMap::from([("subagent_type".to_string(), json!(subagent_type))]),
        );
    }
    let child_permission = match permission_ruleset_for_profile(
        &child_profile,
        task_context.permission_ruleset.clone(),
    ) {
        Ok(value) => value,
        Err(error) => return task_tool_error(tool_call, &error, BTreeMap::new()),
    };
    let (child_provider, child_model) = provider_and_model_for_subagent(
        task_context.provider,
        task_context.model_id,
        &child_profile,
    );
    let mut child_session = match task_id.as_deref() {
        Some(existing) => match task_context.store.load_session(existing) {
            Ok(session) => session,
            Err(error) => {
                return task_tool_error(
                    tool_call,
                    &format!("failed to resume task session {existing}: {error}"),
                    BTreeMap::from([
                        ("subagent_type".to_string(), json!(subagent_type)),
                        ("task_id".to_string(), json!(existing)),
                    ]),
                );
            }
        },
        None => Session::new(new_cli_id("subtask"), task_context.workspace),
    };
    if let Some(existing) = task_id.as_deref() {
        if let Err(error) = validate_task_resume_session(
            &child_session,
            task_context.session,
            &child_profile,
            &subagent_type,
            existing,
        ) {
            return task_tool_error(
                tool_call,
                &error,
                BTreeMap::from([
                    ("subagent_type".to_string(), json!(subagent_type)),
                    ("task_id".to_string(), json!(existing)),
                ]),
            );
        }
    }
    if let Some(error) = subagent_task_governance_error(task_context.session, &child_profile) {
        return task_tool_error(
            tool_call,
            &error,
            BTreeMap::from([
                ("tool".to_string(), json!(TASK_TOOL_ID)),
                ("subagent_type".to_string(), json!(subagent_type)),
                ("status".to_string(), json!("failed")),
                (
                    "task_depth".to_string(),
                    json!(child_task_depth(task_context.session)),
                ),
                (
                    "max_task_depth".to_string(),
                    json!(max_subagent_depth_cli()),
                ),
                (
                    "task_lineage_subagents".to_string(),
                    json!(parent_task_lineage(task_context.session)),
                ),
            ]),
        );
    }
    let task_depth = child_task_depth(task_context.session);
    let task_root_id = task_root_session_id(task_context.session);
    let task_lineage_subagents = child_task_lineage(task_context.session, &child_profile.id);
    let child_run_id = new_cli_id("run");
    let trace_id = new_cli_id("trace");
    child_session.status = SessionStatus::Running;
    child_session
        .metadata
        .insert("agent".to_string(), json!(child_profile.id.clone()));
    child_session
        .metadata
        .insert("provider".to_string(), json!(child_provider.clone()));
    child_session
        .metadata
        .insert("model".to_string(), json!(child_model.clone()));
    child_session.metadata.insert(
        "model_options".to_string(),
        json!(child_profile.model_options.clone()),
    );
    if let Some(temperature) = child_profile.temperature {
        child_session
            .metadata
            .insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = child_profile.top_p {
        child_session
            .metadata
            .insert("top_p".to_string(), json!(top_p));
    }
    if let Some(color) = child_profile.color.as_deref() {
        child_session
            .metadata
            .insert("color".to_string(), json!(color));
    }
    child_session
        .metadata
        .insert("subagent".to_string(), json!(true));
    child_session.metadata.insert(
        "agent_profile".to_string(),
        agent_profile_public_value(&child_profile),
    );
    child_session.metadata.insert(
        "parent_session_id".to_string(),
        json!(task_context.session.id.clone()),
    );
    child_session.metadata.insert(
        "task_parent_session_id".to_string(),
        json!(task_context.session.id.clone()),
    );
    child_session.metadata.insert(
        "task_root_session_id".to_string(),
        json!(task_root_id.clone()),
    );
    child_session
        .metadata
        .insert("task_depth".to_string(), json!(task_depth));
    child_session.metadata.insert(
        "task_lineage_subagents".to_string(),
        json!(task_lineage_subagents.clone()),
    );
    child_session
        .metadata
        .insert("parent_run_id".to_string(), json!(task_context.run_id));
    child_session.metadata.insert(
        "parent_tool_call_id".to_string(),
        json!(tool_call.call_id.clone()),
    );
    child_session
        .metadata
        .insert("task_description".to_string(), json!(description.clone()));
    child_session.metadata.insert(
        "task_subagent_type".to_string(),
        json!(subagent_type.clone()),
    );
    if let Some(command) = command.as_deref() {
        child_session
            .metadata
            .insert("task_command".to_string(), json!(command));
    }
    child_session
        .metadata
        .insert("permission".to_string(), json!(child_permission.as_str()));
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
            .insert("task_resumed_at_ms".to_string(), json!(now_ms_cli()));
    }
    let child_max_steps = child_profile.max_steps.unwrap_or(task_context.max_steps);
    if let Err(error) = task_context.store.start_run(
        &mut child_session,
        StartRunOptions {
            run_id: child_run_id.clone(),
            trace_id,
            agent_name: child_profile.id.clone(),
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
        return task_tool_error(
            tool_call,
            &format!("failed to start subagent session: {error}"),
            BTreeMap::from([("subagent_type".to_string(), json!(subagent_type))]),
        );
    }
    if let Err(error) = bind_agent_profile_system_prompt(
        &mut child_session,
        task_context.store,
        &child_run_id,
        Some(&child_profile),
    ) {
        return task_tool_error(tool_call, &error, BTreeMap::new());
    }
    let user_message = chat_message(Role::User, prompt.clone());
    let user_index = child_session.messages.len() as u64;
    child_session.add(user_message.clone());
    if let Err(error) =
        task_context
            .store
            .append_message(&child_session, &user_message, &child_run_id, user_index)
    {
        return task_tool_error(
            tool_call,
            &format!("failed to record subagent prompt: {error}"),
            BTreeMap::new(),
        );
    }

    let mut child_event_sink: Option<&mut dyn FnMut(&Value)> = None;
    let child_loop_result = run_agent_loop(
        AgentLoopRequest {
            args: task_context.args,
            workspace: task_context.workspace,
            provider: &child_provider,
            model_id: &child_model,
            session: &mut child_session,
            store: task_context.store,
            run_id: &child_run_id,
            max_steps: child_max_steps,
            prompt: &prompt,
            agent_profile: Some(&child_profile),
            permission_ruleset: child_permission.clone(),
            skip_permissions: task_context.skip_permissions,
        },
        &mut child_event_sink,
    );

    match child_loop_result {
        Ok(result) => {
            child_session.status = SessionStatus::Idle;
            let _ = task_context.store.record_event(
                &child_session.id,
                &child_run_id,
                "model.usage",
                SessionEventOptions {
                    kind: "model".to_string(),
                    attributes: BTreeMap::from([
                        ("input_tokens".to_string(), json!(result.usage.input_tokens)),
                        (
                            "output_tokens".to_string(),
                            json!(result.usage.output_tokens),
                        ),
                        ("cost".to_string(), json!(result.usage.cost)),
                        ("source".to_string(), json!(result.source.clone())),
                        ("tool_calls".to_string(), json!(result.tool_calls)),
                    ]),
                    ..SessionEventOptions::default()
                },
            );
            let _ = task_context.store.finish_run(
                &child_session,
                &child_run_id,
                "completed",
                result.steps.max(1),
                Some(&result.finish_reason),
                None,
            );
            let output = render_task_output(&child_session.id, "completed", &result.answer);
            ToolResult {
                call_id: tool_call.call_id.clone(),
                output,
                error: None,
                metadata: BTreeMap::from([
                    ("tool".to_string(), json!(TASK_TOOL_ID)),
                    ("title".to_string(), json!(description)),
                    ("subagent_type".to_string(), json!(subagent_type)),
                    ("task_id".to_string(), json!(child_session.id.clone())),
                    ("session_id".to_string(), json!(child_session.id.clone())),
                    ("run_id".to_string(), json!(child_run_id)),
                    ("status".to_string(), json!("completed")),
                    ("provider".to_string(), json!(child_provider)),
                    ("model".to_string(), json!(child_model)),
                    (
                        "model_options".to_string(),
                        json!(child_profile.model_options.clone()),
                    ),
                    ("task_depth".to_string(), json!(task_depth)),
                    (
                        "task_root_session_id".to_string(),
                        json!(task_root_id.clone()),
                    ),
                    (
                        "task_parent_session_id".to_string(),
                        json!(task_context.session.id.clone()),
                    ),
                    (
                        "task_lineage_subagents".to_string(),
                        json!(task_lineage_subagents.clone()),
                    ),
                    ("steps".to_string(), json!(result.steps)),
                    ("tool_calls".to_string(), json!(result.tool_calls)),
                    (
                        "agent_profile".to_string(),
                        agent_profile_public_value(&child_profile),
                    ),
                ]),
            }
        }
        Err(error) => {
            child_session.status = if error.paused {
                SessionStatus::Paused
            } else {
                SessionStatus::Stop
            };
            let finish_reason = error.finish_reason.as_deref().unwrap_or(if error.paused {
                "paused"
            } else {
                "error"
            });
            let _ = task_context.store.finish_run(
                &child_session,
                &child_run_id,
                "failed",
                error.steps.max(1),
                Some(finish_reason),
                Some(&error.message),
            );
            task_tool_error(
                tool_call,
                &format!("subagent {subagent_type} failed: {}", error.message),
                BTreeMap::from([
                    ("tool".to_string(), json!(TASK_TOOL_ID)),
                    ("title".to_string(), json!(description)),
                    ("subagent_type".to_string(), json!(subagent_type)),
                    ("task_id".to_string(), json!(child_session.id.clone())),
                    ("session_id".to_string(), json!(child_session.id.clone())),
                    ("run_id".to_string(), json!(child_run_id)),
                    (
                        "status".to_string(),
                        json!(if error.paused { "paused" } else { "failed" }),
                    ),
                    ("provider".to_string(), json!(child_provider)),
                    ("model".to_string(), json!(child_model)),
                    (
                        "model_options".to_string(),
                        json!(child_profile.model_options.clone()),
                    ),
                    ("task_depth".to_string(), json!(task_depth)),
                    ("task_root_session_id".to_string(), json!(task_root_id)),
                    (
                        "task_parent_session_id".to_string(),
                        json!(task_context.session.id.clone()),
                    ),
                    (
                        "task_lineage_subagents".to_string(),
                        json!(task_lineage_subagents),
                    ),
                    ("paused".to_string(), json!(error.paused)),
                ]),
            )
        }
    }
}
