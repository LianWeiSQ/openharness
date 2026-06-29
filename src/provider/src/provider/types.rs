#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn protocol_crate_name() -> &'static str {
    openagent_protocol::crate_name()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ProviderStreamEvent {
    TextDelta {
        text: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        input: Value,
    },
    Finish {
        finish_reason: String,
        usage: Usage,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderManager {
    providers: BTreeMap<String, ProviderInfo>,
}

impl ProviderManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_provider(&mut self, provider_id: impl Into<String>, name: impl Into<String>) {
        let id = provider_id.into();
        self.providers.insert(
            id.clone(),
            ProviderInfo {
                id,
                name: name.into(),
            },
        );
    }

    pub fn get_provider(&self, provider_id: &str) -> Result<&ProviderInfo, String> {
        self.providers
            .get(provider_id)
            .ok_or_else(|| format!("Unknown provider: {provider_id}"))
    }

    #[must_use]
    pub fn list_providers(&self) -> Vec<ProviderInfo> {
        self.providers.values().cloned().collect()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenAiLanguageModelConfig {
    pub api_key: String,
    pub model_id: String,
    pub provider_id: String,
    pub base_url: String,
    pub timeout_s: f64,
    pub host_header: Option<String>,
    pub wire_api: String,
    pub reasoning_effort: Option<String>,
    pub disable_response_storage: bool,
}

impl OpenAiLanguageModelConfig {
    #[must_use]
    pub fn new(api_key: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model_id: model_id.into(),
            provider_id: DEFAULT_PROVIDER.to_string(),
            base_url: DEFAULT_OPENAI_BASE_URL.to_string(),
            timeout_s: 60.0,
            host_header: None,
            wire_api: "chat".to_string(),
            reasoning_effort: None,
            disable_response_storage: false,
        }
    }

    #[must_use]
    pub fn chat_headers(&self) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::from([
            ("accept".to_string(), "text/event-stream".to_string()),
            (
                "authorization".to_string(),
                format!("Bearer {}", self.api_key),
            ),
            ("content-type".to_string(), "application/json".to_string()),
        ]);
        if let Some(host_header) = self.host_header.as_ref().filter(|value| !value.is_empty()) {
            headers.insert("host".to_string(), host_header.clone());
        }
        headers
    }

    #[must_use]
    pub fn responses_headers(&self) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::from([
            ("accept".to_string(), "application/json".to_string()),
            (
                "authorization".to_string(),
                format!("Bearer {}", self.api_key),
            ),
            ("content-type".to_string(), "application/json".to_string()),
        ]);
        if let Some(host_header) = self.host_header.as_ref().filter(|value| !value.is_empty()) {
            headers.insert("host".to_string(), host_header.clone());
        }
        headers
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AnthropicLanguageModelConfig {
    pub api_key: String,
    pub model_id: String,
    pub base_url: Option<String>,
    pub timeout_s: f64,
    pub max_output: u64,
}

impl AnthropicLanguageModelConfig {
    #[must_use]
    pub fn new(api_key: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model_id: model_id.into(),
            base_url: None,
            timeout_s: 60.0,
            max_output: DEFAULT_ANTHROPIC_MAX_OUTPUT,
        }
    }
}

#[must_use]
pub fn openai_compatible_model(provider_id: &str, model_id: &str) -> Model {
    let label = provider_label(provider_id).unwrap_or_else(|_| provider_id.to_string());
    Model {
        id: model_id.to_string(),
        provider_id: provider_id.to_string(),
        name: format!("{label} Compatible/{model_id}"),
        context_window: 32_768,
        max_output: 4096,
        capabilities: ModelCapabilities {
            vision: false,
            tools: true,
            streaming: true,
            reasoning: false,
        },
        pricing: ModelPricing::default(),
    }
}

#[must_use]
pub fn anthropic_model(model_id: &str, context_window: u64, max_output: u64) -> Model {
    Model {
        id: model_id.to_string(),
        provider_id: "anthropic".to_string(),
        name: format!("Anthropic/{model_id}"),
        context_window,
        max_output,
        capabilities: ModelCapabilities {
            vision: true,
            tools: true,
            streaming: true,
            reasoning: true,
        },
        pricing: ModelPricing::default(),
    }
}

pub fn normalize_provider(provider: Option<&str>) -> Result<String, String> {
    let raw = provider
        .unwrap_or(DEFAULT_PROVIDER)
        .trim()
        .to_ascii_lowercase();
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return Err(format!("Invalid provider id: {}", provider.unwrap_or("")));
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit())
        || !chars.all(|item| {
            item.is_ascii_lowercase()
                || item.is_ascii_digit()
                || item == '.'
                || item == '_'
                || item == '-'
        })
    {
        return Err(format!("Invalid provider id: {}", provider.unwrap_or("")));
    }
    Ok(raw)
}

pub fn default_env_mapping(provider: &str) -> Result<BTreeMap<String, String>, String> {
    let normalized = normalize_provider(Some(provider))?;
    if normalized == DEFAULT_PROVIDER {
        return Ok(BTreeMap::from([
            ("api_key".to_string(), "OPENAI_API_KEY".to_string()),
            ("base_url".to_string(), "OPENAI_BASE_URL".to_string()),
            ("model".to_string(), "OPENAI_MODEL".to_string()),
            ("wire_api".to_string(), "OPENAI_WIRE_API".to_string()),
        ]));
    }
    let base_provider = normalized
        .split(['.', '_', '-'])
        .next()
        .unwrap_or(normalized.as_str());
    let prefix = provider_env_prefix(&normalized)
        .or_else(|| provider_env_prefix(base_provider))
        .unwrap_or_else(|| {
            let replaced = normalized
                .chars()
                .map(|item| {
                    if item.is_ascii_alphanumeric() {
                        item.to_ascii_uppercase()
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            let trimmed = replaced.trim_matches('_').to_string();
            if trimmed.is_empty() {
                "OPENAGENT".to_string()
            } else {
                trimmed
            }
        });
    Ok(BTreeMap::from([
        ("api_key".to_string(), format!("{prefix}_API_KEY")),
        ("base_url".to_string(), format!("{prefix}_BASE_URL")),
        ("model".to_string(), format!("{prefix}_MODEL")),
        ("wire_api".to_string(), format!("{prefix}_WIRE_API")),
    ]))
}

#[must_use]
pub fn known_provider_ids() -> Vec<&'static str> {
    vec![
        "anthropic",
        "azure-openai",
        "cohere",
        "deepseek",
        "gemini",
        "groq",
        "mistral",
        "ollama",
        "openai",
        "openrouter",
        "xai",
    ]
}

pub fn provider_label(provider: &str) -> Result<String, String> {
    let normalized = normalize_provider(Some(provider))?;
    if let Some(metadata) = provider_metadata(&normalized)
        && let Some(label) = metadata.label
    {
        return Ok(label.to_string());
    }
    Ok(normalized.replace(['_', '.'], "-").to_title_case())
}

pub fn provider_default_base_url(provider: &str) -> Result<Option<String>, String> {
    let normalized = normalize_provider(Some(provider))?;
    Ok(provider_metadata(&normalized)
        .and_then(|metadata| metadata.default_base_url)
        .map(str::to_string))
}

pub fn provider_default_model(provider: &str) -> Result<Option<String>, String> {
    let normalized = normalize_provider(Some(provider))?;
    Ok(provider_metadata(&normalized)
        .and_then(|metadata| metadata.default_model)
        .map(str::to_string))
}

pub fn provider_requires_api_key(provider: &str) -> Result<bool, String> {
    let normalized = normalize_provider(Some(provider))?;
    Ok(provider_metadata(&normalized).is_none_or(|metadata| metadata.requires_api_key))
}

pub fn provider_auth_methods(
    provider: &str,
    present_env: &BTreeSet<String>,
) -> Result<Vec<Value>, String> {
    let normalized = normalize_provider(Some(provider))?;
    let env = default_env_mapping(&normalized)?;
    let requires_key = provider_requires_api_key(&normalized)?;
    let api_key_env = env.get("api_key").cloned().unwrap_or_default();
    let base_url_env = env.get("base_url").cloned().unwrap_or_default();
    let api_key_status = if present_env.contains(&api_key_env) {
        "set"
    } else if !requires_key {
        "not_required"
    } else {
        "missing"
    };
    let mut method = Map::from_iter([
        ("id".to_string(), json!("api_key")),
        ("type".to_string(), json!("api_key")),
        ("label".to_string(), json!("API key")),
        ("provider".to_string(), json!(normalized)),
        (
            "provider_label".to_string(),
            json!(provider_label(provider)?),
        ),
        ("env".to_string(), json!(env)),
        (
            "fields".to_string(),
            json!([
                {"name": "api_key", "env": api_key_env, "required": requires_key, "secret": true},
                {"name": "base_url", "env": base_url_env, "required": false, "secret": false},
                {"name": "model", "env": env.get("model").cloned().unwrap_or_default(), "required": false, "secret": false},
                {"name": "wire_api", "env": env.get("wire_api").cloned().unwrap_or_default(), "required": false, "secret": false},
            ]),
        ),
        ("status".to_string(), json!(api_key_status)),
        (
            "available".to_string(),
            json!(matches!(api_key_status, "set" | "not_required")),
        ),
        ("implemented".to_string(), json!(true)),
    ]);
    if let Some(default_base_url) = provider_default_base_url(provider)? {
        method.insert("default_base_url".to_string(), json!(default_base_url));
    }
    if let Some(default_model) = provider_default_model(provider)? {
        method.insert("default_model".to_string(), json!(default_model));
    }
    if let Some(notes) = provider_metadata(&normalize_provider(Some(provider))?)
        .and_then(|metadata| metadata.auth_notes)
    {
        method.insert("notes".to_string(), json!(notes));
    }
    Ok(vec![Value::Object(method)])
}
