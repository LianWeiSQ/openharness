#[derive(Clone, Debug)]
pub(super) struct ProviderRunResult {
    pub(super) answer: String,
    pub(super) tool_calls: Vec<ToolCall>,
    pub(super) usage: Usage,
    pub(super) source: String,
    pub(super) finish_reason: String,
}
