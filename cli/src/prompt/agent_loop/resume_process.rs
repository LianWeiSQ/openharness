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
