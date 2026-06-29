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
