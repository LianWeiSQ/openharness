//! Shared protocol contracts for OpenAgent.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

pub const RUNTIME_OPTION_KEYS: &[&str] = &[
    "compaction",
    "context_budget",
    "logging",
    "observability",
    "runtime_warnings",
    "session_store",
    "trace",
];

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ModelPricing {
    pub input_per_1m: f64,
    pub output_per_1m: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ModelCapabilities {
    pub vision: bool,
    pub tools: bool,
    pub streaming: bool,
    pub reasoning: bool,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            vision: false,
            tools: true,
            streaming: true,
            reasoning: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Model {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub context_window: u64,
    pub max_output: u64,
    pub capabilities: ModelCapabilities,
    pub pricing: ModelPricing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub name: Option<String>,
    pub tool_call_id: Option<String>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub schema: Option<Value>,
    pub group: String,
    pub dangerous: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolCall {
    pub name: String,
    pub input: Value,
    pub call_id: String,
}

impl ToolCall {
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}:{}", self.name, python_json_dumps(&self.input))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolResult {
    pub call_id: String,
    pub output: String,
    pub error: Option<String>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCall,
    Length,
    Error,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum StreamEvent {
    TextStart {
        id: String,
        metadata: Option<Value>,
    },
    TextDelta {
        id: String,
        text: String,
    },
    TextEnd {
        id: String,
    },
    ToolCall {
        name: String,
        input: Value,
        call_id: String,
    },
    ToolResult {
        call_id: String,
        output: String,
        error: Option<String>,
        metadata: Option<Value>,
    },
    StepStart {
        snapshot_id: String,
    },
    StepFinish {
        tokens: BTreeMap<String, u64>,
        cost: f64,
        finish_reason: FinishReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PermissionRule {
    pub tool: String,
    pub action: PermissionAction,
    pub pattern: Option<String>,
    pub condition: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub enum PermissionRuleset {
    #[serde(rename = "FULL")]
    Full,
    #[serde(rename = "READONLY")]
    Readonly,
    #[serde(rename = "PLAN_ONLY")]
    PlanOnly,
    #[serde(rename = "NONE")]
    None,
}

impl PermissionRuleset {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "FULL",
            Self::Readonly => "READONLY",
            Self::PlanOnly => "PLAN_ONLY",
            Self::None => "NONE",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PermissionRulesetDef {
    pub name: PermissionRuleset,
    pub rules: Vec<PermissionRule>,
}

#[must_use]
pub fn ruleset(name: PermissionRuleset) -> PermissionRulesetDef {
    let rules = match name {
        PermissionRuleset::Full => vec![permission_rule("*", PermissionAction::Allow)],
        PermissionRuleset::Readonly => {
            let mut rules = vec![permission_rule("*", PermissionAction::Deny)];
            rules.extend(
                readonly_tools().map(|tool| permission_rule(tool, PermissionAction::Allow)),
            );
            rules
        }
        PermissionRuleset::PlanOnly => {
            let mut rules = vec![permission_rule("*", PermissionAction::Ask)];
            rules.extend(
                plan_only_tools().map(|tool| permission_rule(tool, PermissionAction::Allow)),
            );
            rules
        }
        PermissionRuleset::None => vec![permission_rule("*", PermissionAction::Deny)],
    };
    PermissionRulesetDef { name, rules }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenAiFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenAiTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OpenAiFunction,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MaterializedPayload {
    pub messages: Vec<Value>,
    pub tools: Vec<OpenAiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_options: BTreeMap<String, Value>,
}

#[must_use]
pub fn materialize_openai_compatible_payload(
    system: Option<&str>,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    model: Option<&Model>,
    options: Option<&BTreeMap<String, Value>>,
) -> MaterializedPayload {
    MaterializedPayload {
        messages: materialize_openai_compatible_messages(system, messages),
        tools: materialize_openai_compatible_tools(tools),
        model: model.map(|item| item.id.clone()),
        provider_options: provider_options(options),
    }
}

#[must_use]
pub fn materialize_openai_compatible_messages(
    system: Option<&str>,
    messages: &[ChatMessage],
) -> Vec<Value> {
    let mut normalized = Vec::new();
    if let Some(system) = system {
        normalized.push(json_object([
            ("role", Value::String("system".to_string())),
            ("content", Value::String(system.to_string())),
        ]));
    }

    for message in messages {
        let role = serde_json::to_value(&message.role).expect("role serializes");
        let mut item = Map::from_iter([
            ("role".to_string(), role),
            (
                "content".to_string(),
                Value::String(message.content.clone()),
            ),
        ]);
        if message.role != Role::Tool
            && let Some(name) = &message.name
        {
            item.insert("name".to_string(), Value::String(name.clone()));
        }
        if let Some(tool_call_id) = &message.tool_call_id {
            item.insert(
                "tool_call_id".to_string(),
                Value::String(tool_call_id.clone()),
            );
        }
        if message.role == Role::Assistant
            && let Some(tool_calls) = message.metadata.get("tool_calls")
            && matches!(tool_calls, Value::Array(items) if !items.is_empty())
        {
            item.insert("tool_calls".to_string(), tool_calls.clone());
            if message.content.is_empty() {
                item.insert("content".to_string(), Value::Null);
            }
        }
        normalized.push(Value::Object(item));
    }
    normalized
}

#[must_use]
pub fn materialize_openai_compatible_tools(tools: &[ToolSchema]) -> Vec<OpenAiTool> {
    tools
        .iter()
        .map(|tool| OpenAiTool {
            kind: "function".to_string(),
            function: OpenAiFunction {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.schema.clone().unwrap_or_else(|| {
                    json_object([("type", Value::String("object".to_string()))])
                }),
            },
        })
        .collect()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Completed,
    Partial,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PermissionMode {
    #[serde(rename = "READONLY")]
    Readonly,
    #[serde(rename = "FULL")]
    Full,
    #[serde(rename = "PLAN_ONLY")]
    PlanOnly,
    #[serde(rename = "NONE")]
    None,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RunLimits {
    pub max_steps: Option<u64>,
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub max_cost: Option<f64>,
    pub timeout_seconds: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FanoutBudget {
    pub max_concurrent: u64,
    pub max_total_workers: u64,
    pub max_total_tokens: Option<u64>,
    pub max_total_cost: Option<f64>,
}

impl Default for FanoutBudget {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            max_total_workers: 8,
            max_total_tokens: None,
            max_total_cost: None,
        }
    }
}

impl FanoutBudget {
    #[must_use]
    pub fn normalized(&self) -> Self {
        Self {
            max_concurrent: self.max_concurrent.max(1),
            max_total_workers: self.max_total_workers.max(1),
            max_total_tokens: self.max_total_tokens,
            max_total_cost: self.max_total_cost,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SwarmUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    pub steps: u64,
    pub latency_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ArtifactRef {
    pub kind: String,
    pub uri: String,
    pub title: String,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentSpec {
    pub role: String,
    pub objective: String,
    pub context: String,
    pub boundaries: String,
    pub output_schema: Value,
    pub inputs: BTreeMap<String, Value>,
    pub limits: RunLimits,
    pub permissions: PermissionMode,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentResult {
    pub status: RunStatus,
    pub summary: String,
    pub evidence: Vec<String>,
    pub open_questions: Vec<String>,
    pub confidence: f64,
    pub artifacts: Vec<ArtifactRef>,
    pub usage: SwarmUsage,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentDescriptor {
    pub id: String,
    pub roles: Vec<String>,
    pub tool_groups: Vec<String>,
    pub model_tier: String,
    pub max_context: u64,
    pub supports_streaming: bool,
    pub kind: String,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunContext {
    pub run_id: String,
    pub parent_span_id: Option<String>,
    pub budget: FanoutBudget,
    pub cancellation: Option<Value>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolConcurrency {
    Safe,
    Exclusive,
    Keyed,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolExecutionSchema {
    pub read_only: bool,
    pub mutates_workspace: bool,
    pub mutates_session: bool,
    pub mutates_external: bool,
    pub external_io: bool,
    pub requires_user_interaction: bool,
    pub concurrency: ToolConcurrency,
    pub batch_group: String,
    pub conflict_key_template: Option<String>,
    pub max_parallelism: Option<u64>,
}

impl ToolExecutionSchema {
    #[must_use]
    pub fn readonly(batch_group: impl Into<String>, max_parallelism: Option<u64>) -> Self {
        Self {
            read_only: true,
            mutates_workspace: false,
            mutates_session: false,
            mutates_external: false,
            external_io: false,
            requires_user_interaction: false,
            concurrency: ToolConcurrency::Safe,
            batch_group: batch_group.into(),
            conflict_key_template: None,
            max_parallelism,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionScope {
    Workspace,
    Agnostic,
    HostOnly,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolDefinitionSchemaFixture {
    pub schema_version: u64,
    pub tool_id: String,
    pub description: String,
    pub group: String,
    pub execution_scope: ToolExecutionScope,
    pub execution_schema: ToolExecutionSchema,
    pub parameters_schema: Value,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkState {
    pub task: String,
    pub progress: Vec<String>,
    pub decisions: Vec<String>,
    pub files: Vec<WorkStateFile>,
    pub tool_findings: Vec<String>,
    pub todos: Vec<String>,
    pub open_questions: Vec<String>,
    pub blockers: Vec<String>,
    pub next_steps: Vec<String>,
    pub risks: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkStateFile {
    pub path: String,
    pub status: String,
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CompactionRecord {
    pub schema_version: u64,
    pub format: String,
    pub state: WorkState,
    pub summary: String,
    pub compacted_until: u64,
    pub updated_at: u64,
    pub source: String,
}

#[must_use]
pub fn render_work_state(state: &WorkState) -> String {
    let mut sections = vec![
        "[Structured work state]".to_string(),
        "Task:".to_string(),
        if state.task.is_empty() {
            "(unspecified)".to_string()
        } else {
            state.task.clone()
        },
    ];

    append_text_section(&mut sections, "Progress", &state.progress);
    append_text_section(&mut sections, "Decisions", &state.decisions);
    append_files_section(&mut sections, &state.files);
    append_text_section(&mut sections, "Tool findings", &state.tool_findings);
    append_text_section(&mut sections, "Todos", &state.todos);
    append_text_section(&mut sections, "Open questions", &state.open_questions);
    append_text_section(&mut sections, "Blockers", &state.blockers);
    append_text_section(&mut sections, "Next steps", &state.next_steps);
    append_text_section(&mut sections, "Risks", &state.risks);

    sections.join("\n").trim().to_string()
}

#[must_use]
pub fn build_compaction_record(
    state: WorkState,
    compacted_until: u64,
    updated_at: u64,
) -> CompactionRecord {
    CompactionRecord {
        schema_version: 1,
        format: "structured_work_state".to_string(),
        summary: render_work_state(&state),
        state,
        compacted_until,
        updated_at,
        source: "model_json".to_string(),
    }
}

fn readonly_tools() -> impl Iterator<Item = &'static str> {
    [
        "read", "glob", "grep", "ls", "skill", "todoread", "question",
    ]
    .into_iter()
}

fn plan_only_tools() -> impl Iterator<Item = &'static str> {
    [
        "read",
        "glob",
        "grep",
        "ls",
        "skill",
        "todoread",
        "todowrite",
        "question",
    ]
    .into_iter()
}

fn permission_rule(tool: &str, action: PermissionAction) -> PermissionRule {
    PermissionRule {
        tool: tool.to_string(),
        action,
        pattern: Some("*".to_string()),
        condition: None,
    }
}

fn provider_options(options: Option<&BTreeMap<String, Value>>) -> BTreeMap<String, Value> {
    let runtime_keys = RUNTIME_OPTION_KEYS.iter().copied().collect::<BTreeSet<_>>();
    options
        .into_iter()
        .flat_map(BTreeMap::iter)
        .filter(|(key, _value)| !runtime_keys.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn append_text_section(lines: &mut Vec<String>, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(format!("{title}:"));
    lines.extend(items.iter().map(|item| format!("- {item}")));
}

fn append_files_section(lines: &mut Vec<String>, files: &[WorkStateFile]) {
    if files.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Files:".to_string());
    lines.extend(
        files
            .iter()
            .map(|file| format!("- {} ({}) - {}", file.path, file.status, file.note)),
    );
}

fn json_object(items: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(Map::from_iter(
        items
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    ))
}

fn python_json_dumps(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => {
            if *value {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("string serializes"),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(python_json_dumps)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        Value::Object(items) => {
            let inner = items
                .iter()
                .map(|(key, value)| {
                    let key = serde_json::to_string(key).expect("key serializes");
                    let value = python_json_dumps(value);
                    format!("{key}: {value}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{inner}}}")
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn exposes_crate_name() {
        assert_eq!(crate_name(), "openagent-protocol");
    }

    #[test]
    fn tool_call_key_matches_python_json_format() {
        let call = ToolCall {
            name: "read".to_string(),
            input: json!({"path": "README.md"}),
            call_id: "call_fixture_read".to_string(),
        };
        assert_eq!(call.key(), "read:{\"path\": \"README.md\"}");
    }
}
