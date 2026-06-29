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
