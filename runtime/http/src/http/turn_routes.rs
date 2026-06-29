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
    let tool_calls = tool_calls_from_turn_payload(&payload)?;
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
    let toolkit = Toolkit::with_builtins();
    let tool_call_count = tool_calls.len() as u64;
    let mut ctx = ToolContext::new(&session.directory)
        .with_session_id(session.id.clone())
        .with_permission_ruleset(permission_ruleset)
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
        let mut tool_result = toolkit.execute(
            &tool_call.name,
            tool_call.input.clone(),
            &tool_call.call_id,
            &mut ctx,
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

fn respond_approval_payload(
    config: &HttpRuntimeConfig,
    path: &str,
    body: &str,
) -> Result<Value, String> {
    let (turn_id, request_id) = parse_turn_approval_path(path)?;
    let payload: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let response = approval_response_payload(&payload)?;
    let action = response
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let store = FileSessionStore::new(session_root(config));
    let mut session = find_session_with_pending_approval(&store, &turn_id, &request_id)?;
    let approval = session
        .metadata
        .get("pending_approval")
        .cloned()
        .ok_or_else(|| "pending approval not found".to_string())?;
    let run_id = approval
        .get("run_id")
        .or_else(|| approval.get("turn_id"))
        .and_then(Value::as_str)
        .unwrap_or(&turn_id)
        .to_string();
    let mut resolved = approval.clone();
    if let Some(object) = resolved.as_object_mut() {
        object.insert("action".to_string(), json!(action));
        object.insert("resolved_at_ms".to_string(), json!(now_ms()));
        if let Some(scope) = response.get("scope") {
            object.insert("scope".to_string(), scope.clone());
        }
        if let Some(note) = response.get("note") {
            object.insert("note".to_string(), note.clone());
        }
    }
    session.metadata.remove("pending_approval");

    let mut events = vec![json!({
        "method": "turn/approval_resolved",
        "params": {
            "session_id": session.id.clone(),
            "thread_id": session.id.clone(),
            "turn_id": run_id.clone(),
            "request_id": request_id.clone(),
            "status": if action == "allow" { "running" } else { "denied" },
            "approval": resolved,
        }
    })];
    let _ = store.record_event(
        &session.id,
        &run_id,
        "approval.resolved",
        SessionEventOptions {
            kind: "approval".to_string(),
            status: action.to_string(),
            attributes: BTreeMap::from([
                ("request_id".to_string(), json!(request_id)),
                ("action".to_string(), json!(action)),
            ]),
            ..SessionEventOptions::default()
        },
    );

    if action == "allow" {
        let tool_call = pending_approval_tool_call(&approval)?;
        let toolkit = Toolkit::with_builtins();
        let mut ctx = ToolContext::new(&session.directory)
            .with_session_id(session.id.clone())
            .with_dangerously_skip_permissions(true);
        let change_before = capture_file_change_before(&session, &tool_call);
        let mut tool_result = toolkit.execute(
            &tool_call.name,
            tool_call.input.clone(),
            &tool_call.call_id,
            &mut ctx,
        );
        append_completed_tool_result(
            &store,
            &mut session,
            &run_id,
            approval.get("step").and_then(Value::as_u64).unwrap_or(1),
            &tool_call,
            change_before,
            &mut tool_result,
            &mut events,
        )?;
        if let Some(resume) = take_pending_provider_turn(&mut session) {
            session.status = SessionStatus::Running;
            return run_provider_loop(RuntimeProviderLoopInput {
                store: &store,
                session: &mut session,
                run_id: &run_id,
                payload: &resume.payload,
                permission_ruleset: resume.permission_ruleset,
                skip_permissions: resume.skip_permissions,
                events,
                carry: resume.carry,
            });
        }
        let failed = tool_result.error.is_some();
        let answer = if failed {
            "approval resolved, but tool execution failed".to_string()
        } else {
            "approval resolved".to_string()
        };
        let input = latest_user_message(&session);
        let usage = usage_payload(&input, &answer, 1);
        let trace = trace_payload(&session, &run_id, 1);
        record_usage_event(&store, &session, &run_id, &usage);
        session.status = SessionStatus::Idle;
        let _ = store.finish_run(
            &session,
            &run_id,
            if failed { "failed" } else { "completed" },
            1,
            Some(if failed { "tool_error" } else { "stop" }),
            None,
        );
        events.push(json!({
            "method": "turn/completed",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "status": if failed { "failed" } else { "completed" },
                "final_answer": answer,
                "usage": usage,
                "trace": trace,
            }
        }));
    } else {
        session.metadata.remove("pending_provider_turn");
        session.status = SessionStatus::Idle;
        let _ = store.finish_run(
            &session,
            &run_id,
            "failed",
            1,
            Some("permission_denied"),
            Some("approval denied"),
        );
        events.push(json!({
            "method": "turn/failed",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id.clone(),
                "status": "failed",
                "error": "approval denied",
            }
        }));
    }
    let _ = store.save_state(&session, Some(&run_id));
    append_app_events(&store.root, &session.id, &run_id, &events);
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "approval": response,
        "events": events,
    }))
}

fn respond_question_payload(
    config: &HttpRuntimeConfig,
    path: &str,
    body: &str,
) -> Result<Value, String> {
    let (turn_id, request_id) = parse_turn_question_reply_path(path)?;
    let payload: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let response = if payload
        .get("dismissed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        question_dismiss_payload(&payload)
    } else {
        question_reply_payload(&payload)?
    };
    let store = FileSessionStore::new(session_root(config));
    let mut session = find_session_with_pending_question(&store, &turn_id, &request_id)?;
    let question = session
        .metadata
        .get("pending_question")
        .cloned()
        .ok_or_else(|| "pending question not found".to_string())?;
    let run_id = question
        .get("run_id")
        .or_else(|| question.get("turn_id"))
        .and_then(Value::as_str)
        .unwrap_or(&turn_id)
        .to_string();
    session.metadata.remove("pending_question");

    let mut events = vec![json!({
        "method": "item/question/resolved",
        "params": {
            "session_id": session.id.clone(),
            "thread_id": session.id.clone(),
            "turn_id": run_id.clone(),
            "request_id": request_id.clone(),
            "status": if response.get("dismissed").and_then(Value::as_bool).unwrap_or(false) {
                "dismissed"
            } else {
                "answered"
            },
            "question": response.clone(),
        }
    })];

    if response
        .get("dismissed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        session.metadata.remove("pending_provider_turn");
        session.status = SessionStatus::Idle;
        let _ = store.finish_run(
            &session,
            &run_id,
            "failed",
            1,
            Some("question_dismissed"),
            Some("question dismissed"),
        );
        events.push(json!({
            "method": "turn/failed",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id.clone(),
                "status": "failed",
                "error": response.get("note").and_then(Value::as_str).unwrap_or("question dismissed"),
            }
        }));
        let _ = store.save_state(&session, Some(&run_id));
        append_app_events(&store.root, &session.id, &run_id, &events);
        return Ok(json!({
            "session_id": session.id,
            "turn_id": run_id,
            "request_id": request_id,
            "question": response,
            "events": events,
        }));
    }

    let tool_call = pending_question_tool_call(&question)?;
    let mut ctx = ToolContext::new(&session.directory).with_session_id(session.id.clone());
    let answers = response
        .get("answers")
        .and_then(question_answers_from_json)
        .unwrap_or_default();
    ctx.set_question_answers(answers);
    let toolkit = Toolkit::with_builtins();
    let mut tool_result = toolkit.execute(
        "question",
        tool_call.input.clone(),
        &tool_call.call_id,
        &mut ctx,
    );
    append_completed_tool_result(
        &store,
        &mut session,
        &run_id,
        question.get("step").and_then(Value::as_u64).unwrap_or(1),
        &tool_call,
        None,
        &mut tool_result,
        &mut events,
    )?;

    if let Some(resume) = take_pending_provider_turn(&mut session) {
        session.status = SessionStatus::Running;
        return run_provider_loop(RuntimeProviderLoopInput {
            store: &store,
            session: &mut session,
            run_id: &run_id,
            payload: &resume.payload,
            permission_ruleset: resume.permission_ruleset,
            skip_permissions: resume.skip_permissions,
            events,
            carry: resume.carry,
        });
    }
    session.status = SessionStatus::Idle;
    let answer = "question answered".to_string();
    let input = latest_user_message(&session);
    let usage = usage_payload(&input, &answer, 1);
    let trace = trace_payload(&session, &run_id, 1);
    record_usage_event(&store, &session, &run_id, &usage);
    let _ = store.finish_run(&session, &run_id, "completed", 1, Some("stop"), None);
    let _ = store.save_state(&session, Some(&run_id));
    events.push(json!({
        "method": "turn/completed",
        "params": {
            "session_id": session.id.clone(),
            "turn_id": run_id.clone(),
            "status": "completed",
            "final_answer": answer,
            "usage": usage,
            "trace": trace,
        }
    }));
    append_app_events(&store.root, &session.id, &run_id, &events);
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "request_id": request_id,
        "question": response,
        "events": events,
    }))
}

fn interrupt_turn_payload(config: &HttpRuntimeConfig, turn_id: &str) -> Result<Value, String> {
    let store = FileSessionStore::new(session_root(config));
    let (session_id, mut session) = find_session_for_turn(&store, turn_id)?;
    session.status = SessionStatus::Stop;
    let _ = store.finish_run(
        &session,
        turn_id,
        "failed",
        1,
        Some("interrupted"),
        Some("interrupt requested"),
    );
    let event = json!({
        "method": "turn/interrupted",
        "params": {
            "session_id": session_id,
            "thread_id": session_id,
            "turn_id": turn_id,
            "status": "interrupted",
            "error": "interrupt requested",
        }
    });
    append_app_events(
        &store.root,
        &session_id,
        turn_id,
        std::slice::from_ref(&event),
    );
    Ok(json!({
        "session_id": session_id,
        "turn_id": turn_id,
        "status": "interrupted",
        "events": [event],
    }))
}

fn enqueue_tui_control_payload(
    config: &HttpRuntimeConfig,
    path: &str,
    body: &str,
) -> Result<Value, String> {
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let request = tui_control_request_for_path(path, &payload)?;
    let mut queue = read_json_array(&tui_control_queue_path(config));
    queue.push(request.to_value());
    write_json_value(&tui_control_queue_path(config), &Value::Array(queue))?;
    Ok(json!({"queued": true, "request": request.to_value()}))
}

fn pop_tui_control_payload(config: &HttpRuntimeConfig) -> Value {
    let path = tui_control_queue_path(config);
    let mut queue = read_json_array(&path);
    if queue.is_empty() {
        return control_next_payload(None);
    }
    let next = queue.remove(0);
    let _ = write_json_value(&path, &Value::Array(queue));
    let request = next.as_object().map(|_| {
        openagent_app_server::TuiControlRequest::new(
            next.get("path").and_then(Value::as_str).unwrap_or_default(),
            next.get("body").cloned().unwrap_or(Value::Null),
        )
    });
    control_next_payload(request.as_ref())
}

fn record_tui_control_response(config: &HttpRuntimeConfig, body: &str) -> Value {
    let payload: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let response = record_control_response_payload(payload);
    append_json_line(&tui_control_responses_path(config), &response);
    response
}

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

fn pending_approval_tool_call(approval: &Value) -> Result<ToolCall, String> {
    let name = approval
        .get("tool_name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "pending approval missing tool_name".to_string())?;
    let call_id = approval
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("call_approval");
    Ok(ToolCall {
        name: name.to_string(),
        input: approval
            .get("tool_input")
            .cloned()
            .unwrap_or_else(|| json!({})),
        call_id: call_id.to_string(),
    })
}

fn pending_question_tool_call(question: &Value) -> Result<ToolCall, String> {
    let name = question
        .get("tool_name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("question");
    let call_id = question
        .get("call_id")
        .or_else(|| question.get("tool_call_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("call_question");
    Ok(ToolCall {
        name: name.to_string(),
        input: question
            .get("tool_input")
            .cloned()
            .unwrap_or_else(|| json!({})),
        call_id: call_id.to_string(),
    })
}

fn approval_payload_for_tool_call(
    session: &Session,
    run_id: &str,
    step: u64,
    call: &ToolCall,
    metadata: &BTreeMap<String, Value>,
) -> Value {
    json!({
        "request_id": format!("approval_{}", call.call_id),
        "session_id": session.id,
        "turn_id": run_id,
        "run_id": run_id,
        "step": step,
        "tool_name": call.name,
        "tool_input": call.input,
        "call_id": call.call_id,
        "created_at_ms": now_ms(),
        "permission_action": metadata.get("permission_action").cloned().unwrap_or_else(|| json!("ask")),
        "permission_pattern": metadata.get("permission_pattern").cloned().unwrap_or_else(|| json!("")),
        "reason": metadata.get("error_kind").cloned().unwrap_or_else(|| json!("permission_required")),
        "metadata": metadata,
    })
}

fn question_payload_for_tool_call(
    session: &Session,
    run_id: &str,
    step: u64,
    call: &ToolCall,
) -> Value {
    json!({
        "request_id": format!("question_{}", call.call_id),
        "session_id": session.id,
        "turn_id": run_id,
        "run_id": run_id,
        "step": step,
        "tool_name": call.name,
        "tool_input": call.input,
        "tool_call_id": call.call_id,
        "call_id": call.call_id,
        "questions": call.input.get("questions").cloned().unwrap_or_else(|| json!([])),
        "created_at_ms": now_ms(),
    })
}

fn question_answers_from_json(value: &Value) -> Option<Vec<Vec<String>>> {
    let items = value.as_array()?;
    if items.iter().all(Value::is_array) {
        return Some(
            items
                .iter()
                .map(|item| {
                    item.as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(value_to_answer_string)
                        .collect::<Vec<_>>()
                })
                .collect(),
        );
    }
    Some(
        items
            .iter()
            .filter_map(value_to_answer_string)
            .map(|answer| vec![answer])
            .collect(),
    )
}

fn value_to_answer_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_bool().map(|item| item.to_string()))
        .or_else(|| value.as_i64().map(|item| item.to_string()))
        .or_else(|| value.as_u64().map(|item| item.to_string()))
        .or_else(|| value.as_f64().map(|item| item.to_string()))
}

fn tool_calls_from_turn_payload(payload: &Value) -> Result<Vec<ToolCall>, String> {
    if let Some(tool_call) = payload.get("tool_call") {
        return Ok(vec![tool_call_from_value(tool_call, 0)?]);
    }
    if let Some(items) = payload.get("tool_calls").and_then(Value::as_array) {
        return items
            .iter()
            .enumerate()
            .map(|(index, item)| tool_call_from_value(item, index))
            .collect();
    }
    Ok(Vec::new())
}

fn tool_call_from_value(value: &Value, index: usize) -> Result<ToolCall, String> {
    let name = value
        .get("name")
        .or_else(|| value.get("tool"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "tool call name is required".to_string())?;
    Ok(ToolCall {
        name: name.to_string(),
        input: value
            .get("input")
            .or_else(|| value.get("arguments"))
            .cloned()
            .unwrap_or_else(|| json!({})),
        call_id: value
            .get("call_id")
            .or_else(|| value.get("id"))
            .and_then(Value::as_str)
            .map_or_else(|| format!("call_{index}"), str::to_string),
    })
}

fn permission_ruleset_for_turn(payload: &Value) -> Result<PermissionRuleset, String> {
    let raw = payload
        .get("permission")
        .or_else(|| payload.get("permissions"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| std::env::var("OPENAGENT_APP_PERMISSION").ok())
        .unwrap_or_else(|| "FULL".to_string());
    parse_permission_ruleset(&raw)
}

fn parse_permission_ruleset(raw: &str) -> Result<PermissionRuleset, String> {
    match raw.trim().to_ascii_uppercase().replace('-', "_").as_str() {
        "FULL" | "ALLOW" | "AUTO" => Ok(PermissionRuleset::Full),
        "READONLY" | "READ_ONLY" => Ok(PermissionRuleset::Readonly),
        "PLAN_ONLY" | "ASK" => Ok(PermissionRuleset::PlanOnly),
        "NONE" | "DENY" => Ok(PermissionRuleset::None),
        _ => Err("permission must be FULL, READONLY, PLAN_ONLY, or NONE".to_string()),
    }
}

fn skip_permissions_for_turn(payload: &Value) -> bool {
    payload
        .get("dangerously_skip_permissions")
        .or_else(|| payload.get("skip_permissions"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            std::env::var("OPENAGENT_APP_DANGEROUSLY_SKIP_PERMISSIONS")
                .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes"))
        })
}

fn provider_streaming_enabled_for_turn(payload: &Value) -> bool {
    payload
        .get("stream")
        .or_else(|| payload.get("provider_stream"))
        .or_else(|| payload.get("stream_provider"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            std::env::var("OPENAGENT_PROVIDER_STREAM")
                .map(|value| !matches!(value.as_str(), "0" | "false" | "FALSE" | "no"))
                .unwrap_or(true)
        })
}
