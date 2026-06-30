fn start_turn_payload(
    config: &HttpRuntimeConfig,
    session_id: &str,
    body: &str,
) -> Result<Value, String> {
    let payload: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let input = payload
        .get("input")
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if input.trim().is_empty() {
        return Err("turn input is required".to_string());
    }
    let permission_ruleset = permission_ruleset_for_turn(&payload)?;
    let skip_permissions = skip_permissions_for_turn(&payload);
    let store = FileSessionStore::new(session_root(config));
    let mut session = store
        .load_session(session_id)
        .unwrap_or_else(|_| Session::new(session_id.to_string(), workspace(config)));
    let runtime_profile = apply_turn_runtime_profile(&mut session, &payload);
    let run_id = new_id("turn");
    session.status = SessionStatus::Running;
    let _ = store.start_run(
        &mut session,
        StartRunOptions {
            run_id: run_id.clone(),
            trace_id: new_id("trace"),
            agent_name: runtime_profile.agent.clone(),
            model_id: Some(runtime_profile.model.clone()),
            provider_id: Some("openagent".to_string()),
            permission: if skip_permissions {
                format!("auto_allow:{}", permission_ruleset.as_str())
            } else {
                permission_ruleset.as_str().to_string()
            },
            max_steps: 1,
            started_at_ms: None,
        },
    );
    let user = ChatMessage {
        role: Role::User,
        content: input.to_string(),
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::new(),
    };
    let user_index = session.messages.len() as u64;
    session.add(user.clone());
    let _ = store.append_message(&session, &user, &run_id, user_index);
    let mut tool_calls = tool_calls_from_turn_payload(&payload)?;
    if tool_calls.is_empty()
        && let Some(call) = manual_runtime_subagent_tool_call(&input)
    {
        tool_calls.push(call);
    }
    if !tool_calls.is_empty() {
        return run_http_tool_turn(
            &store,
            &mut session,
            &run_id,
            tool_calls,
            permission_ruleset,
            skip_permissions,
        );
    }
    let _ = runtime_profile;
    let initial_events = vec![turn_started_event(&session, &run_id)];
    run_provider_loop(RuntimeProviderLoopInput {
        store: &store,
        session: &mut session,
        run_id: &run_id,
        payload: &payload,
        permission_ruleset,
        skip_permissions,
        events: initial_events,
        carry: RuntimeProviderLoopCarry::default(),
    })
}

fn run_http_tool_turn(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    tool_calls: Vec<ToolCall>,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
) -> Result<Value, String> {
    let agent_profile = runtime_agent_profile_for_session(session);
    let toolkit = toolkit_with_runtime_task_tool(session, agent_profile.as_ref());
    let tool_call_count = tool_calls.len() as u64;
    let empty_payload = json!({});
    let mut ctx = ToolContext::new(&session.directory)
        .with_session_id(session.id.clone())
        .with_permission_manager(runtime_permission_manager_for_agent(
            permission_ruleset.clone(),
            agent_profile.as_ref(),
        ))
        .with_dangerously_skip_permissions(skip_permissions);
    let mut events = vec![turn_started_event(session, run_id)];

    for (index, tool_call) in tool_calls.into_iter().enumerate() {
        let step = index as u64 + 1;
        events.push(json!({
            "method": "item/toolCall/started",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "call_id": tool_call.call_id.clone(),
                "name": tool_call.name.clone(),
                "input": tool_call.input.clone(),
            }
        }));
        let change_before = capture_file_change_before(session, &tool_call);
        let mut tool_result = execute_runtime_tool_call(
            &toolkit,
            &tool_call,
            &mut ctx,
            RuntimeTaskExecutionContext {
                store,
                parent_session: session,
                parent_run_id: run_id,
                payload: &empty_payload,
                skip_permissions,
            },
        );
        if tool_result
            .metadata
            .get("requires_approval")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let mut approval = approval_payload_for_tool_call(
                session,
                run_id,
                step,
                &tool_call,
                &tool_result.metadata,
            );
            if let Some(preview) = change_before
                .as_ref()
                .and_then(|before| file_change_preview(before, &tool_call))
                && let Some(object) = approval.as_object_mut()
            {
                object.insert("preview".to_string(), preview);
            }
            session.status = SessionStatus::Paused;
            session
                .metadata
                .insert("pending_approval".to_string(), approval.clone());
            let _ = store.record_event(
                &session.id,
                run_id,
                "approval.requested",
                SessionEventOptions {
                    kind: "approval".to_string(),
                    attributes: BTreeMap::from([
                        ("call_id".to_string(), json!(tool_call.call_id)),
                        ("name".to_string(), json!(tool_call.name)),
                        ("approval".to_string(), approval.clone()),
                    ]),
                    ..SessionEventOptions::default()
                },
            );
            let _ = store.save_state(session, Some(run_id));
            events.push(json!({
                "method": "turn/approval_requested",
                "params": {
                    "session_id": session.id.clone(),
                    "turn_id": run_id,
                    "status": "waiting_approval",
                    "approval": approval,
                }
            }));
            append_app_events(&store.root, &session.id, run_id, &events);
            return Ok(json!({
                "session_id": session.id,
                "turn_id": run_id,
                "status": "waiting_approval",
                "events": events,
            }));
        }

        let failed = tool_result.error.is_some();
        let patch = complete_file_change(
            store,
            session,
            run_id,
            &tool_call,
            change_before,
            &tool_result,
        );
        if let Some(change) = patch.as_ref() {
            tool_result
                .metadata
                .insert("patch".to_string(), public_file_change(change));
            tool_result.metadata.insert(
                "patch_id".to_string(),
                change.get("id").cloned().unwrap_or(Value::Null),
            );
            tool_result.metadata.insert(
                "diff".to_string(),
                change.get("diff").cloned().unwrap_or(Value::Null),
            );
        }
        events.push(json!({
            "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "call_id": tool_call.call_id.clone(),
                "name": tool_call.name.clone(),
                "output": tool_result.output,
                "error": tool_result.error,
                "metadata": tool_result.metadata,
            }
        }));
        if let Some(change) = patch.as_ref() {
            events.push(patch_detected_event(session, run_id, change));
        }
    }

    let answer = if tool_calls_completed_successfully(&events) {
        "tool execution completed".to_string()
    } else {
        "tool execution failed".to_string()
    };
    let assistant = ChatMessage {
        role: Role::Assistant,
        content: answer.clone(),
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::new(),
    };
    let assistant_index = session.messages.len() as u64;
    session.add(assistant.clone());
    session.status = SessionStatus::Idle;
    let _ = store.append_message(session, &assistant, run_id, assistant_index);
    let _ = store.finish_run(session, run_id, "completed", 1, Some("stop"), None);
    let input = latest_user_message(session);
    let usage = usage_payload(&input, &answer, tool_call_count);
    let trace = trace_payload(session, run_id, tool_call_count);
    record_usage_event(store, session, run_id, &usage);
    events.push(json!({
        "method": "turn/completed",
        "params": {
            "thread_id": session.id.clone(),
            "turn_id": run_id,
            "status": "completed",
            "final_answer": answer,
            "usage": usage,
            "trace": trace,
        }
    }));
    append_app_events(&store.root, &session.id, run_id, &events);
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "status": "completed",
        "events": events,
    }))
}
