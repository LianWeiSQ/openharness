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
