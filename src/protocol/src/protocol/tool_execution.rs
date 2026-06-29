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
