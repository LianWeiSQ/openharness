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
        let agent_profile = runtime_agent_profile_for_session(&session);
        let toolkit = toolkit_with_runtime_task_tool(&session, agent_profile.as_ref());
        let pending_payload = session
            .metadata
            .get("pending_provider_turn")
            .and_then(|pending| pending.get("payload"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let pending_skip_permissions = session
            .metadata
            .get("pending_provider_turn")
            .and_then(|pending| pending.get("skip_permissions"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut ctx = ToolContext::new(&session.directory)
            .with_session_id(session.id.clone())
            .with_permission_manager(runtime_permission_manager_for_agent(
                parse_permission_ruleset(
                    session
                        .metadata
                        .get("permission")
                        .and_then(Value::as_str)
                        .unwrap_or("FULL"),
                )
                .unwrap_or(PermissionRuleset::Full),
                agent_profile.as_ref(),
            ))
            .with_dangerously_skip_permissions(true);
        let change_before = capture_file_change_before(&session, &tool_call);
        let mut tool_result = execute_runtime_tool_call(
            &toolkit,
            &tool_call,
            &mut ctx,
            RuntimeTaskExecutionContext {
                store: &store,
                parent_session: &session,
                parent_run_id: &run_id,
                payload: &pending_payload,
                skip_permissions: pending_skip_permissions,
            },
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
    let agent_profile = runtime_agent_profile_for_session(&session);
    let toolkit = toolkit_with_runtime_task_tool(&session, agent_profile.as_ref());
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
