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
        format!("{}:{}", self.name, stable_json_dumps(&self.input))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolResult {
    pub call_id: String,
    pub output: String,
    pub error: Option<String>,
    pub metadata: BTreeMap<String, Value>,
}
