#[derive(Clone, Debug)]
struct RuntimeProviderResult {
    answer: String,
    tool_calls: Vec<ToolCall>,
    usage: Usage,
    source: String,
    finish_reason: String,
}

struct OpenAiRuntimeProviderRequest<'a> {
    provider: &'a str,
    model: &'a str,
    api_key: &'a str,
    base_url: &'a str,
    wire_api: &'a str,
    timeout_s: u64,
    stream: bool,
    messages: &'a [ChatMessage],
    tools: &'a [openagent_protocol::ToolSchema],
}

#[derive(Clone, Debug)]
struct RuntimeProviderLoopCarry {
    answer: String,
    usage: Usage,
    tool_calls: u64,
    next_step: u64,
}

impl Default for RuntimeProviderLoopCarry {
    fn default() -> Self {
        Self {
            answer: String::new(),
            usage: Usage::default(),
            tool_calls: 0,
            next_step: 1,
        }
    }
}

#[derive(Clone, Debug)]
struct RuntimeProviderResume {
    payload: Value,
    carry: RuntimeProviderLoopCarry,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
}

struct RuntimeProviderLoopInput<'a> {
    store: &'a FileSessionStore,
    session: &'a mut Session,
    run_id: &'a str,
    payload: &'a Value,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
    events: Vec<Value>,
    carry: RuntimeProviderLoopCarry,
}
