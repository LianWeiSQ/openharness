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
