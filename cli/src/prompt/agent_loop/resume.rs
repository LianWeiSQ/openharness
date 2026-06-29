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
