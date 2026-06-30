struct ManualSubagentRunContext<'a, 'sink> {
    args: &'a [String],
    workspace: &'a Path,
    provider: &'a str,
    model_id: &'a str,
    toolkit: &'a Toolkit,
    mcp_runtime: Option<&'a McpRuntime>,
    ctx: &'a mut ToolContext,
    session: &'a mut Session,
    store: &'a FileSessionStore,
    run_id: &'a str,
    max_steps: u64,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
    events: &'a mut Vec<Value>,
    event_sink: &'a mut Option<&'sink mut dyn FnMut(&Value)>,
    total_tool_calls: u64,
}

fn run_manual_subagent_turn(
    tool_call: ToolCall,
    context: &mut ManualSubagentRunContext<'_, '_>,
) -> Result<AgentLoopOutcome, AgentLoopError> {
    emit_run_event(
        context.events,
        json!({
            "method": "item/toolCall/started",
            "params": {
                "session_id": context.session.id.clone(),
                "run_id": context.run_id,
                "step": 1,
                "call_id": tool_call.call_id.clone(),
                "name": tool_call.name.clone(),
                "input": tool_call.input.clone(),
                "manual": true,
            }
        }),
        context.event_sink,
    );
    let mut assistant = assistant_message_for_provider_step(String::new(), &[tool_call.clone()]);
    assistant.metadata.insert(
        "message_id".to_string(),
        json!(cli_message_id(context.session.messages.len() as u64)),
    );
    assistant.metadata.insert("step".to_string(), json!(1));
    let assistant_index = context.session.messages.len() as u64;
    context.session.add(assistant.clone());
    context
        .store
        .append_message(context.session, &assistant, context.run_id, assistant_index)
        .map_err(|error| AgentLoopError {
            message: format!("failed to record manual subagent call: {error}"),
            events: context.events.clone(),
            steps: 1,
            finish_reason: Some("store_error".to_string()),
            paused: false,
        })?;
    let tool_result = execute_loop_tool_call(
        context.toolkit,
        context.mcp_runtime,
        &tool_call,
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
    let failed = tool_result.error.is_some();
    emit_run_event(
        context.events,
        json!({
            "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
            "params": {
                "session_id": context.session.id.clone(),
                "run_id": context.run_id,
                "step": 1,
                "call_id": tool_call.call_id.clone(),
                "name": tool_call.name.clone(),
                "output": tool_result.output.clone(),
                "error": tool_result.error.clone(),
                "metadata": tool_result.metadata.clone(),
                "manual": true,
            }
        }),
        context.event_sink,
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
    let tool_index = context.session.messages.len() as u64;
    context.session.add(tool_message.clone());
    context
        .store
        .append_message(context.session, &tool_message, context.run_id, tool_index)
        .map_err(|error| AgentLoopError {
            message: format!("failed to record manual subagent result: {error}"),
            events: context.events.clone(),
            steps: 1,
            finish_reason: Some("store_error".to_string()),
            paused: false,
        })?;
    let final_answer = tool_result
        .error
        .clone()
        .unwrap_or_else(|| tool_result.output.clone());
    Ok(AgentLoopOutcome {
        answer: final_answer,
        usage: Usage::default(),
        source: "manual_subagent".to_string(),
        events: std::mem::take(context.events),
        steps: 1,
        tool_calls: context.total_tool_calls,
        finish_reason: if failed { "tool_error" } else { "stop" }.to_string(),
    })
}
