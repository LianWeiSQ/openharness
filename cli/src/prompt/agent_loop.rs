use super::tool::{
    add_approval_always_pattern, approval_always_patterns, approval_payload_for_tool_call,
    assistant_message_for_provider_step, configured_question_answers, execute_agent_tool,
    question_answers_from_json, value_to_answer_string,
};
use super::*;
use openagent_tools::TASK_TOOL_ID;

#[derive(Debug)]
pub(super) struct AgentLoopOutcome {
    pub(super) answer: String,
    pub(super) usage: Usage,
    pub(super) source: String,
    pub(super) events: Vec<Value>,
    pub(super) steps: u64,
    pub(super) tool_calls: u64,
    pub(super) finish_reason: String,
}

#[derive(Debug)]
pub(super) struct AgentLoopError {
    pub(super) message: String,
    pub(super) events: Vec<Value>,
    pub(super) steps: u64,
    pub(super) finish_reason: Option<String>,
    pub(super) paused: bool,
}

#[derive(Clone, Debug)]
pub(super) struct PendingResume {
    pub(super) kind: String,
    pub(super) request_id: String,
    pub(super) call: ToolCall,
    pub(super) response: Value,
    pub(super) step: u64,
}

pub(super) struct AgentLoopRequest<'a> {
    pub(super) args: &'a [String],
    pub(super) workspace: &'a Path,
    pub(super) provider: &'a str,
    pub(super) model_id: &'a str,
    pub(super) session: &'a mut Session,
    pub(super) store: &'a FileSessionStore,
    pub(super) run_id: &'a str,
    pub(super) max_steps: u64,
    pub(super) prompt: &'a str,
    pub(super) agent_profile: Option<&'a RunAgentProfile>,
    pub(super) permission_ruleset: PermissionRuleset,
    pub(super) skip_permissions: bool,
}

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
    register_task_tool(&mut toolkit.registry, &task_subagent_descriptors(args));
    let tools = filter_tools_for_agent(toolkit.get_all_tools("local"), agent_profile);
    let mut ctx = ToolContext::new(workspace)
        .with_session_id(session.id.clone())
        .with_permission_ruleset(permission_ruleset.clone())
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

struct TaskExecutionContext<'a> {
    args: &'a [String],
    workspace: &'a Path,
    provider: &'a str,
    model_id: &'a str,
    session: &'a Session,
    store: &'a FileSessionStore,
    run_id: &'a str,
    max_steps: u64,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
}

fn execute_loop_tool_call(
    toolkit: &Toolkit,
    mcp_runtime: Option<&McpRuntime>,
    tool_call: &ToolCall,
    ctx: &mut ToolContext,
    task_context: TaskExecutionContext<'_>,
) -> ToolResult {
    if tool_call.name == TASK_TOOL_ID {
        execute_task_tool_call(toolkit, tool_call, ctx, task_context)
    } else {
        execute_agent_tool(toolkit, mcp_runtime, tool_call, ctx)
    }
}

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
                    ("paused".to_string(), json!(error.paused)),
                ]),
            )
        }
    }
}

fn task_input_string(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("task tool requires non-empty {key}"))
}

fn task_tool_error(
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

fn render_task_output(task_id: &str, state: &str, text: &str) -> String {
    format!(
        "<task id=\"{}\" state=\"{}\">\n<task_result>\n{}\n</task_result>\n</task>",
        escape_task_text(task_id),
        escape_task_text(state),
        escape_task_text(text),
    )
}

fn escape_task_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(super) fn pending_resume_from_session(session: &Session) -> Option<PendingResume> {
    if let Some(response) = session.metadata.get("pending_question_response")
        && let Some(pending) = session.metadata.get("pending_question")
    {
        return pending_resume_from_values("question", pending, response);
    }
    if let Some(response) = session.metadata.get("pending_approval_response")
        && let Some(pending) = session.metadata.get("pending_approval")
    {
        return pending_resume_from_values("approval", pending, response);
    }
    None
}

fn pending_resume_from_values(
    kind: &str,
    pending: &Value,
    response: &Value,
) -> Option<PendingResume> {
    let call_id = pending.get("call_id").and_then(Value::as_str)?.to_string();
    let tool_name = pending
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or(if kind == "question" { "question" } else { "" })
        .to_string();
    if tool_name.is_empty() {
        return None;
    }
    Some(PendingResume {
        kind: kind.to_string(),
        request_id: pending
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or(&call_id)
            .to_string(),
        call: ToolCall {
            name: tool_name,
            input: pending
                .get("tool_input")
                .or_else(|| pending.get("toolInput"))
                .cloned()
                .unwrap_or_else(|| json!({})),
            call_id,
        },
        response: response.clone(),
        step: pending.get("step").and_then(Value::as_u64).unwrap_or(0),
    })
}

pub(super) struct PendingResumeContext<'a, 'sink> {
    pub(super) args: &'a [String],
    pub(super) workspace: &'a Path,
    pub(super) provider: &'a str,
    pub(super) model_id: &'a str,
    pub(super) toolkit: &'a Toolkit,
    pub(super) mcp_runtime: Option<&'a McpRuntime>,
    pub(super) ctx: &'a mut ToolContext,
    pub(super) session: &'a mut Session,
    pub(super) store: &'a FileSessionStore,
    pub(super) run_id: &'a str,
    pub(super) max_steps: u64,
    pub(super) permission_ruleset: PermissionRuleset,
    pub(super) skip_permissions: bool,
    pub(super) events: &'a mut Vec<Value>,
    pub(super) event_sink: &'a mut Option<&'sink mut dyn FnMut(&Value)>,
}

fn process_pending_resume(
    pending: PendingResume,
    context: &mut PendingResumeContext<'_, '_>,
) -> Result<(), String> {
    emit_run_event(
        context.events,
        json!({
            "method": format!("turn/{}_resumed", pending.kind),
            "params": {
                "session_id": context.session.id.clone(),
                "run_id": context.run_id,
                "request_id": pending.request_id.clone(),
                "call_id": pending.call.call_id.clone(),
            }
        }),
        context.event_sink,
    );
    let result = if pending.kind == "question" {
        let answers = pending
            .response
            .get("answers")
            .and_then(question_answers_from_json)
            .or_else(|| {
                pending
                    .response
                    .get("answer")
                    .and_then(value_to_answer_string)
                    .map(|answer| vec![vec![answer]])
            })
            .unwrap_or_default();
        context.ctx.set_question_answers(answers);
        context.toolkit.execute(
            "question",
            pending.call.input.clone(),
            &pending.call.call_id,
            context.ctx,
        )
    } else {
        let decision = pending
            .response
            .get("decision")
            .and_then(Value::as_str)
            .unwrap_or("allow_once");
        if matches!(decision, "reject" | "deny") {
            ToolResult {
                call_id: pending.call.call_id.clone(),
                output: String::new(),
                error: Some(
                    pending
                        .response
                        .get("note")
                        .and_then(Value::as_str)
                        .unwrap_or("Permission rejected by user")
                        .to_string(),
                ),
                metadata: BTreeMap::from([
                    ("tool".to_string(), json!(pending.call.name.clone())),
                    ("permission_action".to_string(), json!("reject")),
                    ("request_id".to_string(), json!(pending.request_id.clone())),
                ]),
            }
        } else {
            if matches!(decision, "allow_always" | "always")
                && let Some(pattern) = context
                    .session
                    .metadata
                    .get("pending_approval")
                    .and_then(|item| item.get("permission_pattern"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            {
                add_approval_always_pattern(context.session, pattern);
            }
            let previous = context.ctx.dangerously_skip_permissions;
            context.ctx.dangerously_skip_permissions = true;
            let result = execute_loop_tool_call(
                context.toolkit,
                context.mcp_runtime,
                &pending.call,
                context.ctx,
                TaskExecutionContext {
                    args: context.args,
                    workspace: context.workspace,
                    provider: context.provider,
                    model_id: context.model_id,
                    session: context.session,
                    store: context.store,
                    run_id: context.run_id,
                    max_steps: context.max_steps,
                    permission_ruleset: context.permission_ruleset.clone(),
                    skip_permissions: context.skip_permissions,
                },
            );
            context.ctx.dangerously_skip_permissions = previous;
            result
        }
    };
    append_tool_result_to_session(context, pending.step, &pending.call, result)?;
    context.session.metadata.remove("pending_question");
    context.session.metadata.remove("pending_question_response");
    context.session.metadata.remove("pending_approval");
    context.session.metadata.remove("pending_approval_response");
    context
        .store
        .save_state(context.session, Some(context.run_id))
        .map_err(|error| format!("failed to save resumed session state: {error}"))?;
    Ok(())
}

fn append_tool_result_to_session(
    context: &mut PendingResumeContext<'_, '_>,
    step: u64,
    tool_call: &ToolCall,
    tool_result: ToolResult,
) -> Result<(), String> {
    let failed = tool_result.error.is_some();
    emit_run_event(
        context.events,
        json!({
            "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
            "params": {
                "session_id": context.session.id.clone(),
                "run_id": context.run_id,
                "step": step,
                "call_id": tool_call.call_id.clone(),
                "name": tool_call.name.clone(),
                "output": tool_result.output.clone(),
                "error": tool_result.error.clone(),
                "metadata": tool_result.metadata.clone(),
            }
        }),
        context.event_sink,
    );
    let _ = context.store.record_event(
        &context.session.id,
        context.run_id,
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
    let _ = context.store.append_part(
        &context.session.id,
        context.run_id,
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
    if let Some(message_id) = context
        .session
        .metadata
        .get(if tool_call.name == "question" {
            "pending_question"
        } else {
            "pending_approval"
        })
        .and_then(|value| value.get("assistant_message_id"))
        .and_then(Value::as_str)
    {
        tool_message
            .metadata
            .insert("assistant_message_id".to_string(), json!(message_id));
    }
    tool_message
        .metadata
        .insert("step".to_string(), json!(step));
    let tool_index = context.session.messages.len() as u64;
    context.session.add(tool_message.clone());
    context
        .store
        .append_message(context.session, &tool_message, context.run_id, tool_index)
        .map_err(|error| format!("failed to record resumed tool message: {error}"))
}

fn emit_run_event(
    events: &mut Vec<Value>,
    event: Value,
    event_sink: &mut Option<&mut dyn FnMut(&Value)>,
) {
    if let Some(emit) = event_sink.as_deref_mut() {
        emit(&event);
    }
    events.push(event);
}

fn record_step_finished(
    store: &FileSessionStore,
    session_id: &str,
    run_id: &str,
    step: u64,
    finish_reason: &str,
    tool_calls: u64,
    usage: &Usage,
) {
    let _ = store.record_event(
        session_id,
        run_id,
        "step.finished",
        SessionEventOptions {
            kind: "step".to_string(),
            attributes: BTreeMap::from([
                ("step".to_string(), json!(step)),
                ("finish_reason".to_string(), json!(finish_reason)),
                ("tool_calls".to_string(), json!(tool_calls)),
                ("input_tokens".to_string(), json!(usage.input_tokens)),
                ("output_tokens".to_string(), json!(usage.output_tokens)),
            ]),
            ..SessionEventOptions::default()
        },
    );
}
