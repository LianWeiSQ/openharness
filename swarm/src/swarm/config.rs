#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SwarmConfig {
    #[serde(default)]
    pub fanout_budget: FanoutBudget,
    #[serde(default, deserialize_with = "deserialize_runner_configs")]
    pub runners: Vec<RunnerConfig>,
    #[serde(default, deserialize_with = "deserialize_task_configs")]
    pub tasks: Vec<TaskConfig>,
}

impl SwarmConfig {
    #[must_use]
    pub fn task(&self, task_id: &str) -> Option<TaskConfig> {
        self.tasks.iter().find(|task| task.id == task_id).cloned()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RunnerConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default = "default_function_kind")]
    pub kind: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tool_groups: Vec<String>,
    #[serde(default = "default_model_tier")]
    pub model_tier: String,
    #[serde(default)]
    pub max_context: u64,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl RunnerConfig {
    #[must_use]
    pub fn to_descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            id: self.id.clone(),
            roles: self.roles.clone(),
            tool_groups: self.tool_groups.clone(),
            model_tier: self.model_tier.clone(),
            max_context: self.max_context,
            supports_streaming: self.supports_streaming,
            kind: self.kind.clone(),
            metadata: self.metadata.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TaskConfig {
    #[serde(default)]
    pub id: String,
    pub role: String,
    pub objective: String,
    pub context: String,
    pub boundaries: String,
    pub output_schema: Value,
    #[serde(default)]
    pub runner_ids: Vec<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub limits: RunLimits,
    #[serde(default = "default_permission_mode")]
    pub permissions: PermissionMode,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResultPayload {
    Text(String),
    Json(Value),
    Result(AgentResult),
}

impl From<&str> for ResultPayload {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<String> for ResultPayload {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<Value> for ResultPayload {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}

impl From<AgentResult> for ResultPayload {
    fn from(value: AgentResult) -> Self {
        Self::Result(value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwarmRunResult {
    pub task_id: String,
    pub status: String,
    pub summary: String,
    pub results: BTreeMap<String, AgentResult>,
    pub usage: SwarmUsage,
    pub warnings: Vec<String>,
    pub events: Vec<AgentEvent>,
}
