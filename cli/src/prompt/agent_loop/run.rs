pub(super) fn run_agent_loop(
    request: AgentLoopRequest<'_>,
    event_sink: &mut Option<&mut dyn FnMut(&Value)>,
) -> Result<AgentLoopOutcome, AgentLoopError> {
    let AgentLoopRequest {
        args,
        workspace,
        provider,
        model_id,
        session,
        store,
        run_id,
        max_steps,
        prompt,
        agent_profile,
        permission_ruleset,
        skip_permissions,
    } = request;
    let mut toolkit = Toolkit::with_builtins();
    let mcp_runtime = load_mcp_runtime(args, &mut toolkit).map_err(|message| AgentLoopError {
        message,
        events: Vec::new(),
        steps: 0,
        finish_reason: Some("mcp_discovery_error".to_string()),
        paused: false,
    })?;
    register_task_tool(
        &mut toolkit.registry,
        &task_subagent_descriptors(args, agent_profile, Some(session)),
    );
    let tools = filter_tools_for_agent(toolkit.get_all_tools("local"), agent_profile);
    let mut ctx = ToolContext::new(workspace)
        .with_session_id(session.id.clone())
        .with_permission_manager(permission_manager_for_agent(
            permission_ruleset.clone(),
            agent_profile,
        ))
        .with_dangerously_skip_permissions(skip_permissions);
    if let Some(answers) = configured_question_answers(args) {
        ctx.set_question_answers(answers);
    }
    if let Some(runtime) = mcp_runtime.as_ref() {
        let _ = store.record_event(
            &session.id,
            run_id,
            "mcp.discovery",
            SessionEventOptions {
                kind: "mcp".to_string(),
                attributes: BTreeMap::from([(
                    "snapshot".to_string(),
                    sanitize_mcp_observation_value(&runtime.snapshot),
                )]),
                ..SessionEventOptions::default()
            },
        );
    }

    let mut answer = String::new();
    let mut events = Vec::new();
    let mut total_usage = Usage::default();
    let mut total_tool_calls = 0_u64;
    let mut first_delta = true;
    let mut approval_always = approval_always_patterns(session);

    if let Some(pending) = pending_resume_from_session(session) {
        total_tool_calls += 1;
        let mut resume_context = PendingResumeContext {
            args,
            workspace,
            provider,
            model_id,
            toolkit: &toolkit,
            mcp_runtime: mcp_runtime.as_ref(),
            ctx: &mut ctx,
            session,
            store,
            run_id,
            max_steps,
            permission_ruleset: permission_ruleset.clone(),
            skip_permissions,
            events: &mut events,
            event_sink,
        };
        process_pending_resume(pending, &mut resume_context).map_err(|message| AgentLoopError {
            message,
            events: events.clone(),
            steps: 0,
            finish_reason: Some("resume_error".to_string()),
            paused: false,
        })?;
        approval_always = approval_always_patterns(session);
    }

    if let Some(tool_call) = manual_subagent_tool_call(prompt) {
        total_tool_calls += 1;
        emit_run_event(
            &mut events,
            json!({
                "method": "item/toolCall/started",
                "params": {
                    "session_id": session.id.clone(),
                    "run_id": run_id,
                    "step": 1,
                    "call_id": tool_call.call_id.clone(),
                    "name": tool_call.name.clone(),
                    "input": tool_call.input.clone(),
                    "manual": true,
                }
            }),
            event_sink,
        );
        let mut assistant =
            assistant_message_for_provider_step(String::new(), &[tool_call.clone()]);
        assistant.metadata.insert(
            "message_id".to_string(),
            json!(cli_message_id(session.messages.len() as u64)),
        );
        assistant.metadata.insert("step".to_string(), json!(1));
        let assistant_index = session.messages.len() as u64;
        session.add(assistant.clone());
        store
            .append_message(session, &assistant, run_id, assistant_index)
            .map_err(|error| AgentLoopError {
                message: format!("failed to record manual subagent call: {error}"),
                events: events.clone(),
                steps: 1,
                finish_reason: Some("store_error".to_string()),
                paused: false,
            })?;
        let tool_result = execute_loop_tool_call(
            &toolkit,
            mcp_runtime.as_ref(),
            &tool_call,
            &mut ctx,
            TaskExecutionContext {
                args,
                workspace,
                provider,
                model_id,
                session,
                store,
                run_id,
                max_steps,
                permission_ruleset: permission_ruleset.clone(),
                skip_permissions,
            },
        );
        let failed = tool_result.error.is_some();
        emit_run_event(
            &mut events,
            json!({
                "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
                "params": {
                    "session_id": session.id.clone(),
                    "run_id": run_id,
                    "step": 1,
                    "call_id": tool_call.call_id.clone(),
                    "name": tool_call.name.clone(),
                    "output": tool_result.output.clone(),
                    "error": tool_result.error.clone(),
                    "metadata": tool_result.metadata.clone(),
                    "manual": true,
                }
            }),
            event_sink,
        );
        let mut tool_message = chat_message(
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
        tool_message.metadata.insert("step".to_string(), json!(1));
        let tool_index = session.messages.len() as u64;
        session.add(tool_message.clone());
        store
            .append_message(session, &tool_message, run_id, tool_index)
            .map_err(|error| AgentLoopError {
                message: format!("failed to record manual subagent result: {error}"),
                events: events.clone(),
                steps: 1,
                finish_reason: Some("store_error".to_string()),
                paused: false,
            })?;
        let final_answer = tool_result
            .error
            .clone()
            .unwrap_or_else(|| tool_result.output.clone());
        return Ok(AgentLoopOutcome {
            answer: final_answer,
            usage: total_usage,
            source: "manual_subagent".to_string(),
            events,
            steps: 1,
            tool_calls: total_tool_calls,
            finish_reason: if failed { "tool_error" } else { "stop" }.to_string(),
        });
    }

    for step in 1..=max_steps {
        let mut streamed_events = Vec::new();
        let provider_messages = store
            .materialized_chat_messages(session)
            .unwrap_or_else(|_| session.messages.clone());
        let mut on_provider_stream = |event: &ProviderStreamEvent| {
            if let ProviderStreamEvent::TextDelta { text } = event
                && !text.is_empty()
            {
                let mut params = json!({
                    "delta": text,
                    "session_id": session.id.clone(),
                    "run_id": run_id,
                    "step": step,
                });
                if first_delta {
                    params["prompt"] = json!(prompt);
                    first_delta = false;
                }
                emit_run_event(
                    &mut streamed_events,
                    json!({"method": "item/agentMessage/delta", "params": params}),
                    event_sink,
                );
            }
        };
        let provider_result = call_provider_for_run(
            args,
            provider,
            model_id,
            &provider_messages,
            &tools,
            Some(&mut on_provider_stream),
            agent_profile,
        )
        .map_err(|message| AgentLoopError {
            message,
            events: events.clone(),
            steps: step,
            finish_reason: Some("provider_error".to_string()),
            paused: false,
        })?;
        let streamed_text = !streamed_events.is_empty();
        events.extend(streamed_events);
        let source = provider_result.source.clone();
        add_usage(&mut total_usage, &provider_result.usage);
        let step_text = provider_result.answer.clone();
        if !step_text.is_empty() {
            answer.push_str(&step_text);
            if !streamed_text {
                let mut params = json!({
                    "delta": step_text,
                    "session_id": session.id.clone(),
                    "run_id": run_id,
                    "step": step,
                });
                if first_delta {
                    params["prompt"] = json!(prompt);
                    first_delta = false;
                }
                emit_run_event(
                    &mut events,
                    json!({"method": "item/agentMessage/delta", "params": params}),
                    event_sink,
                );
            }
            store
                .append_part(
                    &session.id,
                    run_id,
                    "text",
                    SessionPartOptions {
                        attributes: BTreeMap::from([
                            ("role".to_string(), json!("assistant")),
                            ("chars".to_string(), json!(step_text.chars().count())),
                        ]),
                        step_index: Some(step),
                        ..SessionPartOptions::default()
                    },
                )
                .map_err(|error| AgentLoopError {
                    message: format!("failed to record assistant text part: {error}"),
                    events: events.clone(),
                    steps: step,
                    finish_reason: Some("store_error".to_string()),
                    paused: false,
                })?;
        }

        let assistant_index = session.messages.len() as u64;
        let assistant_message_id = cli_message_id(assistant_index);
        let mut assistant_message =
            assistant_message_for_provider_step(step_text, &provider_result.tool_calls);
        assistant_message.metadata.insert(
            "message_id".to_string(),
            json!(assistant_message_id.clone()),
        );
        assistant_message
            .metadata
            .insert("step".to_string(), json!(step));
        session.add(assistant_message.clone());
        store
            .append_message(session, &assistant_message, run_id, assistant_index)
            .map_err(|error| AgentLoopError {
                message: format!("failed to record assistant message: {error}"),
                events: events.clone(),
                steps: step,
                finish_reason: Some("store_error".to_string()),
                paused: false,
            })?;

        if provider_result.tool_calls.is_empty() {
            record_step_finished(
                store,
                &session.id,
                run_id,
                step,
                &provider_result.finish_reason,
                0,
                &provider_result.usage,
            );
            return Ok(AgentLoopOutcome {
                answer,
                usage: total_usage,
                source,
                events,
                steps: step,
                tool_calls: total_tool_calls,
                finish_reason: provider_result.finish_reason,
            });
        }

        for tool_call in provider_result.tool_calls {
            total_tool_calls += 1;
            emit_run_event(
                &mut events,
                json!({
                    "method": "item/toolCall/started",
                    "params": {
                        "session_id": session.id.clone(),
                        "run_id": run_id,
                        "step": step,
                        "call_id": tool_call.call_id.clone(),
                        "name": tool_call.name.clone(),
                        "input": tool_call.input.clone(),
                    }
                }),
                event_sink,
            );
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
                let message =
                    "question tool requires an answer; rerun with --answer or OPENAGENT_QUESTION_ANSWERS".to_string();
                emit_run_event(
                    &mut events,
                    json!({
                        "method": "turn/question_requested",
                        "params": {
                            "session_id": session.id.clone(),
                            "run_id": run_id,
                            "step": step,
                            "call_id": tool_call.call_id.clone(),
                            "questions": tool_call.input.get("questions").cloned().unwrap_or_else(|| json!([])),
                        }
                    }),
                    event_sink,
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
                let _ = store.append_part(
                    &session.id,
                    run_id,
                    "question",
                    SessionPartOptions {
                        message_id: Some(assistant_message_id.clone()),
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
                session.metadata.insert(
                    "pending_question".to_string(),
                    json!({
                        "request_id": format!("question_{}", tool_call.call_id),
                        "session_id": session.id.clone(),
                        "turn_id": run_id,
                        "run_id": run_id,
                        "step": step,
                        "call_id": tool_call.call_id.clone(),
                        "tool_name": tool_call.name.clone(),
                        "tool_input": tool_call.input.clone(),
                        "assistant_message_id": assistant_message_id.clone(),
                        "questions": tool_call.input.get("questions").cloned().unwrap_or_else(|| json!([])),
                        "created_at_ms": now_ms_cli(),
                    }),
                );
                session.metadata.remove("pending_question_response");
                let _ = store.save_state(session, Some(run_id));
                return Err(AgentLoopError {
                    message,
                    events,
                    steps: step,
                    finish_reason: Some("question_required".to_string()),
                    paused: true,
                });
            }

            let mut tool_result = execute_loop_tool_call(
                &toolkit,
                mcp_runtime.as_ref(),
                &tool_call,
                &mut ctx,
                TaskExecutionContext {
                    args,
                    workspace,
                    provider,
                    model_id,
                    session,
                    store,
                    run_id,
                    max_steps,
                    permission_ruleset: permission_ruleset.clone(),
                    skip_permissions,
                },
            );
            if tool_result
                .metadata
                .get("requires_approval")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let pattern = tool_result
                    .metadata
                    .get("permission_pattern")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if approval_always.iter().any(|item| item == &pattern) {
                    let previous = ctx.dangerously_skip_permissions;
                    ctx.dangerously_skip_permissions = true;
                    tool_result = execute_loop_tool_call(
                        &toolkit,
                        mcp_runtime.as_ref(),
                        &tool_call,
                        &mut ctx,
                        TaskExecutionContext {
                            args,
                            workspace,
                            provider,
                            model_id,
                            session,
                            store,
                            run_id,
                            max_steps,
                            permission_ruleset: permission_ruleset.clone(),
                            skip_permissions,
                        },
                    );
                    ctx.dangerously_skip_permissions = previous;
                }
            }
            if tool_result
                .metadata
                .get("requires_approval")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let message = format!(
                    "approval required for tool {} (call {})",
                    tool_call.name, tool_call.call_id
                );
                let mut approval = approval_payload_for_tool_call(
                    session,
                    run_id,
                    step,
                    &tool_call,
                    &tool_result.metadata,
                );
                if let Some(object) = approval.as_object_mut() {
                    object.insert(
                        "assistant_message_id".to_string(),
                        json!(assistant_message_id.clone()),
                    );
                }
                emit_run_event(
                    &mut events,
                    json!({
                        "method": "turn/approval_requested",
                        "params": {
                            "session_id": session.id.clone(),
                            "run_id": run_id,
                            "step": step,
                            "approval": approval,
                        }
                    }),
                    event_sink,
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
                            (
                                "reason".to_string(),
                                json!(
                                    tool_result
                                        .metadata
                                        .get("error_kind")
                                        .and_then(Value::as_str)
                                        .unwrap_or("permission_required")
                                ),
                            ),
                            ("metadata".to_string(), json!(tool_result.metadata)),
                        ]),
                        ..SessionEventOptions::default()
                    },
                );
                let _ = store.append_part(
                    &session.id,
                    run_id,
                    "approval",
                    SessionPartOptions {
                        message_id: Some(assistant_message_id.clone()),
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
                session
                    .metadata
                    .insert("pending_approval".to_string(), approval.clone());
                session.metadata.remove("pending_approval_response");
                let _ = store.save_state(session, Some(run_id));
                return Err(AgentLoopError {
                    message,
                    events,
                    steps: step,
                    finish_reason: Some("approval_required".to_string()),
                    paused: true,
                });
            }
            let failed = tool_result.error.is_some();
            let tool_output = tool_result.output.clone();
            let tool_error = tool_result.error.clone();
            let tool_metadata = tool_result.metadata.clone();
            emit_run_event(
                &mut events,
                json!({
                    "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
                    "params": {
                        "session_id": session.id.clone(),
                        "run_id": run_id,
                        "step": step,
                        "call_id": tool_call.call_id.clone(),
                        "name": tool_call.name.clone(),
                        "output": tool_output,
                        "error": tool_error,
                        "metadata": tool_metadata,
                    }
                }),
                event_sink,
            );
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

            let mut tool_message = chat_message(
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
            tool_message.metadata.insert(
                "assistant_message_id".to_string(),
                json!(assistant_message_id.clone()),
            );
            tool_message
                .metadata
                .insert("step".to_string(), json!(step));
            let tool_index = session.messages.len() as u64;
            session.add(tool_message.clone());
            store
                .append_message(session, &tool_message, run_id, tool_index)
                .map_err(|error| AgentLoopError {
                    message: format!("failed to record tool message: {error}"),
                    events: events.clone(),
                    steps: step,
                    finish_reason: Some("store_error".to_string()),
                    paused: false,
                })?;
        }

        record_step_finished(
            store,
            &session.id,
            run_id,
            step,
            "tool_call",
            total_tool_calls,
            &provider_result.usage,
        );
    }

    Err(AgentLoopError {
        message: format!("agent loop exceeded max steps ({max_steps})"),
        events,
        steps: max_steps,
        finish_reason: Some("max_steps".to_string()),
        paused: false,
    })
}

fn manual_subagent_tool_call(prompt: &str) -> Option<ToolCall> {
    let trimmed = prompt.trim_start();
    let rest = trimmed.strip_prefix('@')?;
    let (subagent_type, task_prompt) = rest.split_once(char::is_whitespace)?;
    let subagent_type = subagent_type.trim();
    let task_prompt = task_prompt.trim();
    if subagent_type.is_empty()
        || task_prompt.is_empty()
        || !subagent_type
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return None;
    }
    Some(ToolCall {
        name: TASK_TOOL_ID.to_string(),
        input: json!({
            "description": format!("@{subagent_type}"),
            "prompt": task_prompt,
            "subagent_type": subagent_type,
        }),
        call_id: format!("manual_task_{subagent_type}"),
    })
}
