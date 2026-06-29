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
