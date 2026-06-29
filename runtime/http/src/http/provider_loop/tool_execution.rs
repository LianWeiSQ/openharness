#[allow(clippy::too_many_arguments)]
fn execute_provider_tool_call(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    payload: &Value,
    step: u64,
    tool_call: &ToolCall,
    toolkit: &Toolkit,
    ctx: &mut ToolContext,
    permission_ruleset: &PermissionRuleset,
    skip_permissions: bool,
    pending_carry: &RuntimeProviderLoopCarry,
    events: &mut Vec<Value>,
    persisted_events: &mut usize,
) -> Result<Option<Value>, String> {
    events.push(json!({
        "method": "item/toolCall/started",
        "params": {
            "session_id": session.id.clone(),
            "turn_id": run_id,
            "run_id": run_id,
            "step": step,
            "call_id": tool_call.call_id.clone(),
            "name": tool_call.name.clone(),
            "input": tool_call.input.clone(),
        }
    }));
    append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
    let _ = store.record_event(
        &session.id,
        run_id,
        "tool.call.started",
        SessionEventOptions {
            kind: "tool".to_string(),
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("input".to_string(), tool_call.input.clone()),
                ("step".to_string(), json!(step)),
            ]),
            ..SessionEventOptions::default()
        },
    );

    if tool_call.name == "question" && ctx.question_answers.is_none() {
        let question = question_payload_for_tool_call(session, run_id, step, tool_call);
        session.status = SessionStatus::Paused;
        session
            .metadata
            .insert("pending_question".to_string(), question.clone());
        session.metadata.remove("pending_question_response");
        store_pending_provider_turn(
            session,
            payload,
            pending_carry,
            permission_ruleset.clone(),
            skip_permissions,
        );
        let _ = store.record_event(
            &session.id,
            run_id,
            "question.requested",
            SessionEventOptions {
                kind: "question".to_string(),
                attributes: BTreeMap::from([
                    ("call_id".to_string(), json!(tool_call.call_id.clone())),
                    (
                        "questions".to_string(),
                        tool_call
                            .input
                            .get("questions")
                            .cloned()
                            .unwrap_or_else(|| json!([])),
                    ),
                ]),
                ..SessionEventOptions::default()
            },
        );
        if let Some(message_id) = latest_assistant_message_id_for_tool(session, tool_call) {
            let _ = store.append_part(
                &session.id,
                run_id,
                "question",
                SessionPartOptions {
                    message_id: Some(message_id),
                    content: Some(json!({
                        "call_id": tool_call.call_id.clone(),
                        "name": tool_call.name.clone(),
                        "questions": tool_call.input.get("questions").cloned().unwrap_or_else(|| json!([])),
                        "status": "pending",
                    })),
                    attributes: BTreeMap::from([
                        ("call_id".to_string(), json!(tool_call.call_id.clone())),
                        ("name".to_string(), json!(tool_call.name.clone())),
                    ]),
                    step_index: Some(step),
                    status: "pending".to_string(),
                    ..SessionPartOptions::default()
                },
            );
        }
        let _ = store.save_state(session, Some(run_id));
        events.push(json!({
            "method": "item/question/requested",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "status": "waiting_question",
                "event": question,
            }
        }));
        append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
        return Ok(Some(json!({
            "session_id": session.id,
            "turn_id": run_id,
            "status": "waiting_question",
            "events": events,
        })));
    }

    let change_before = capture_file_change_before(session, tool_call);
    let mut tool_result = toolkit.execute(
        &tool_call.name,
        tool_call.input.clone(),
        &tool_call.call_id,
        ctx,
    );
    if tool_result
        .metadata
        .get("requires_approval")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut approval =
            approval_payload_for_tool_call(session, run_id, step, tool_call, &tool_result.metadata);
        if let Some(preview) = change_before
            .as_ref()
            .and_then(|before| file_change_preview(before, tool_call))
            && let Some(object) = approval.as_object_mut()
        {
            object.insert("preview".to_string(), preview);
        }
        session.status = SessionStatus::Paused;
        session
            .metadata
            .insert("pending_approval".to_string(), approval.clone());
        session.metadata.remove("pending_approval_response");
        store_pending_provider_turn(
            session,
            payload,
            pending_carry,
            permission_ruleset.clone(),
            skip_permissions,
        );
        let _ = store.record_event(
            &session.id,
            run_id,
            "approval.requested",
            SessionEventOptions {
                kind: "approval".to_string(),
                attributes: BTreeMap::from([
                    ("call_id".to_string(), json!(tool_call.call_id.clone())),
                    ("name".to_string(), json!(tool_call.name.clone())),
                    ("approval".to_string(), approval.clone()),
                ]),
                ..SessionEventOptions::default()
            },
        );
        if let Some(message_id) = latest_assistant_message_id_for_tool(session, tool_call) {
            let _ = store.append_part(
                &session.id,
                run_id,
                "approval",
                SessionPartOptions {
                    message_id: Some(message_id),
                    content: Some(json!({
                        "call_id": tool_call.call_id.clone(),
                        "name": tool_call.name.clone(),
                        "approval": approval.clone(),
                        "status": "pending",
                    })),
                    attributes: BTreeMap::from([
                        ("call_id".to_string(), json!(tool_call.call_id.clone())),
                        ("name".to_string(), json!(tool_call.name.clone())),
                    ]),
                    step_index: Some(step),
                    status: "pending".to_string(),
                    ..SessionPartOptions::default()
                },
            );
        }
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
        append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
        return Ok(Some(json!({
            "session_id": session.id,
            "turn_id": run_id,
            "status": "waiting_approval",
            "events": events,
        })));
    }

    append_completed_tool_result(
        store,
        session,
        run_id,
        step,
        tool_call,
        change_before,
        &mut tool_result,
        events,
    )?;
    append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn append_completed_tool_result(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    step: u64,
    tool_call: &ToolCall,
    change_before: Option<FileChangeBefore>,
    tool_result: &mut ToolResult,
    events: &mut Vec<Value>,
) -> Result<(), String> {
    let failed = tool_result.error.is_some();
    let patch = complete_file_change(
        store,
        session,
        run_id,
        tool_call,
        change_before,
        tool_result,
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
            "run_id": run_id,
            "step": step,
            "call_id": tool_call.call_id.clone(),
            "name": tool_call.name.clone(),
            "output": tool_result.output.clone(),
            "error": tool_result.error.clone(),
            "metadata": tool_result.metadata.clone(),
        }
    }));
    if let Some(change) = patch.as_ref() {
        events.push(patch_detected_event(session, run_id, change));
    }
    append_tool_result_to_session(store, session, run_id, step, tool_call, tool_result)
}

fn append_tool_result_to_session(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    step: u64,
    tool_call: &ToolCall,
    tool_result: &ToolResult,
) -> Result<(), String> {
    let failed = tool_result.error.is_some();
    let _ = store.record_event(
        &session.id,
        run_id,
        if failed {
            "tool.call.failed"
        } else {
            "tool.call.finished"
        },
        SessionEventOptions {
            kind: "tool".to_string(),
            status: if failed {
                "error".to_string()
            } else {
                "ok".to_string()
            },
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("error".to_string(), json!(tool_result.error.clone())),
                ("metadata".to_string(), json!(tool_result.metadata.clone())),
                ("step".to_string(), json!(step)),
            ]),
            ..SessionEventOptions::default()
        },
    );
    let _ = store.append_part(
        &session.id,
        run_id,
        "tool_result",
        SessionPartOptions {
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("failed".to_string(), json!(failed)),
            ]),
            step_index: Some(step),
            ..SessionPartOptions::default()
        },
    );
    let mut tool_message = runtime_chat_message(
        Role::Tool,
        tool_result.error.as_ref().map_or_else(
            || tool_result.output.clone(),
            |error| format!("Tool failed: {error}"),
        ),
    );
    tool_message.name = Some(tool_call.name.clone());
    tool_message.tool_call_id = Some(tool_call.call_id.clone());
    tool_message
        .metadata
        .insert("tool_result".to_string(), json!(tool_result));
    tool_message
        .metadata
        .insert("step".to_string(), json!(step));
    if let Some(message_id) = latest_assistant_message_id_for_tool(session, tool_call) {
        tool_message
            .metadata
            .insert("assistant_message_id".to_string(), json!(message_id));
    }
    let tool_index = session.messages.len() as u64;
    session.add(tool_message.clone());
    store
        .append_message(session, &tool_message, run_id, tool_index)
        .map_err(|error| format!("failed to record tool message: {error}"))
}
