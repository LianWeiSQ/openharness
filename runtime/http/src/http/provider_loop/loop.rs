fn run_provider_loop(input: RuntimeProviderLoopInput<'_>) -> Result<Value, String> {
    let RuntimeProviderLoopInput {
        store,
        session,
        run_id,
        payload,
        permission_ruleset,
        skip_permissions,
        mut events,
        mut carry,
    } = input;
    let max_steps = provider_max_steps(payload);
    let toolkit = Toolkit::with_builtins();
    let mut ctx = ToolContext::new(&session.directory)
        .with_session_id(session.id.clone())
        .with_permission_ruleset(permission_ruleset.clone())
        .with_dangerously_skip_permissions(skip_permissions);
    if let Some(answers) = payload
        .get("question_answers")
        .or_else(|| payload.get("answers"))
        .and_then(question_answers_from_json)
    {
        ctx.set_question_answers(answers);
    }

    let mut persisted_events = 0;
    append_unpersisted_app_events(
        &store.root,
        &session.id,
        run_id,
        &events,
        &mut persisted_events,
    );
    while carry.next_step <= max_steps {
        let step = carry.next_step;
        let mut streamed_text = false;
        let session_id = session.id.clone();
        let root = store.root.clone();
        let mut on_provider_stream = |event: &ProviderStreamEvent| {
            if let ProviderStreamEvent::TextDelta { text } = event
                && !text.is_empty()
            {
                streamed_text = true;
                events.push(json!({
                    "method": "item/agentMessage/delta",
                    "params": {
                        "thread_id": session_id.clone(),
                        "session_id": session_id.clone(),
                        "turn_id": run_id,
                        "run_id": run_id,
                        "step": step,
                        "event": {"id": format!("assistant_{step}"), "text": text.clone()},
                        "delta": text.clone(),
                    }
                }));
                append_unpersisted_app_events(
                    &root,
                    &session_id,
                    run_id,
                    &events,
                    &mut persisted_events,
                );
            }
        };
        let provider_result =
            provider_turn_result(store, session, payload, Some(&mut on_provider_stream))?;
        add_usage(&mut carry.usage, &provider_result.usage);
        if provider_result.source == "provider_missing_api_key" {
            events.push(json!({
                "method": "runtime/warning",
                "params": {
                    "session_id": session.id.clone(),
                    "turn_id": run_id,
                    "message": provider_result.answer.clone(),
                    "code": "provider_missing_api_key",
                }
            }));
        }
        if !provider_result.answer.is_empty() {
            carry.answer.push_str(&provider_result.answer);
            if !streamed_text {
                events.push(json!({
                    "method": "item/agentMessage/delta",
                    "params": {
                        "thread_id": session.id.clone(),
                        "session_id": session.id.clone(),
                        "turn_id": run_id,
                        "run_id": run_id,
                        "step": step,
                        "event": {"id": format!("assistant_{step}"), "text": provider_result.answer.clone()},
                        "delta": provider_result.answer.clone(),
                    }
                }));
            }
            let _ = store.append_part(
                &session.id,
                run_id,
                "text",
                SessionPartOptions {
                    attributes: BTreeMap::from([
                        ("role".to_string(), json!("assistant")),
                        (
                            "chars".to_string(),
                            json!(provider_result.answer.chars().count()),
                        ),
                    ]),
                    step_index: Some(step),
                    ..SessionPartOptions::default()
                },
            );
        }

        let assistant_index = session.messages.len() as u64;
        let assistant_message_id = runtime_message_id(assistant_index);
        let mut assistant = assistant_message_for_provider_step(
            provider_result.answer.clone(),
            &provider_result.tool_calls,
        );
        assistant
            .metadata
            .insert("message_id".to_string(), json!(assistant_message_id));
        assistant.metadata.insert("step".to_string(), json!(step));
        session.add(assistant.clone());
        let _ = store.append_message(session, &assistant, run_id, assistant_index);

        if provider_result.tool_calls.is_empty() {
            return finish_provider_loop(
                store,
                session,
                run_id,
                events,
                &mut persisted_events,
                carry,
                &provider_result.finish_reason,
            );
        }

        let resume_carry = RuntimeProviderLoopCarry {
            next_step: step.saturating_add(1),
            ..carry.clone()
        };
        for tool_call in &provider_result.tool_calls {
            carry.tool_calls = carry.tool_calls.saturating_add(1);
            let pending_carry = RuntimeProviderLoopCarry {
                tool_calls: carry.tool_calls,
                next_step: step.saturating_add(1),
                ..resume_carry.clone()
            };
            if let Some(paused) = execute_provider_tool_call(
                store,
                session,
                run_id,
                payload,
                step,
                tool_call,
                &toolkit,
                &mut ctx,
                &permission_ruleset,
                skip_permissions,
                &pending_carry,
                &mut events,
                &mut persisted_events,
            )? {
                return Ok(paused);
            }
        }

        carry.next_step = step.saturating_add(1);
    }

    session.status = SessionStatus::Idle;
    let _ = store.finish_run(
        session,
        run_id,
        "failed",
        max_steps,
        Some("max_steps"),
        Some("agent loop exceeded max_steps"),
    );
    let usage = usage_value_from_provider(
        &carry.usage,
        carry.tool_calls,
        &latest_user_message(session),
        &carry.answer,
    );
    let trace = trace_payload(session, run_id, carry.tool_calls);
    events.push(json!({
        "method": "turn/failed",
        "params": {
            "session_id": session.id.clone(),
            "turn_id": run_id,
            "status": "failed",
            "error": "agent loop exceeded max_steps",
            "usage": usage,
            "trace": trace,
        }
    }));
    append_unpersisted_app_events(
        &store.root,
        &session.id,
        run_id,
        &events,
        &mut persisted_events,
    );
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "status": "failed",
        "events": events,
    }))
}
