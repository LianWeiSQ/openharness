fn provider_env_prefix(provider: &str) -> Option<String> {
    match provider {
        "anthropic" => Some("ANTHROPIC"),
        "azure" | "azure-openai" => Some("AZURE_OPENAI"),
        "cohere" => Some("COHERE"),
        "deepseek" => Some("DEEPSEEK"),
        "gemini" | "google" => Some("GOOGLE"),
        "groq" => Some("GROQ"),
        "mistral" => Some("MISTRAL"),
        "ollama" => Some("OLLAMA"),
        "openai" => Some("OPENAI"),
        "openrouter" => Some("OPENROUTER"),
        "xai" => Some("XAI"),
        _ => None,
    }
    .map(str::to_string)
}

fn provider_metadata(provider: &str) -> Option<ProviderMetadata> {
    let metadata = match provider {
        "anthropic" => ProviderMetadata {
            label: Some("Anthropic"),
            default_base_url: None,
            default_model: Some(DEFAULT_ANTHROPIC_MODEL),
            requires_api_key: true,
            auth_notes: Some(
                "Native Anthropic Messages routing is supported with ANTHROPIC_API_KEY; well-known provider URL login remains tracked separately.",
            ),
        },
        "azure-openai" => ProviderMetadata {
            label: Some("Azure OpenAI"),
            default_base_url: None,
            default_model: Some(DEFAULT_OPENAI_MODEL),
            requires_api_key: true,
            auth_notes: Some(
                "Set AZURE_OPENAI_BASE_URL to your deployment endpoint when using the OpenAI-compatible runtime.",
            ),
        },
        "cohere" => ProviderMetadata {
            label: Some("Cohere"),
            default_base_url: None,
            default_model: Some("command-a-03-2025"),
            requires_api_key: true,
            auth_notes: Some(
                "Native Cohere SDK routing is not implemented; use an OpenAI-compatible gateway/base URL for runtime calls.",
            ),
        },
        "deepseek" => ProviderMetadata {
            label: Some("DeepSeek"),
            default_base_url: Some("https://api.deepseek.com/v1"),
            default_model: Some("deepseek-chat"),
            requires_api_key: true,
            auth_notes: None,
        },
        "gemini" => ProviderMetadata {
            label: Some("Google Gemini"),
            default_base_url: None,
            default_model: Some("gemini-2.5-pro"),
            requires_api_key: true,
            auth_notes: Some(
                "Native Gemini SDK routing is not implemented; use an OpenAI-compatible gateway/base URL for runtime calls.",
            ),
        },
        "groq" => ProviderMetadata {
            label: Some("Groq"),
            default_base_url: Some("https://api.groq.com/openai/v1"),
            default_model: Some("llama-3.3-70b-versatile"),
            requires_api_key: true,
            auth_notes: None,
        },
        "mistral" => ProviderMetadata {
            label: Some("Mistral"),
            default_base_url: Some("https://api.mistral.ai/v1"),
            default_model: Some("mistral-large-latest"),
            requires_api_key: true,
            auth_notes: None,
        },
        "ollama" => ProviderMetadata {
            label: Some("Ollama"),
            default_base_url: Some("http://localhost:11434/v1"),
            default_model: Some("llama3.2"),
            requires_api_key: false,
            auth_notes: None,
        },
        "openai" => ProviderMetadata {
            label: Some("OpenAI"),
            default_base_url: Some(DEFAULT_OPENAI_BASE_URL),
            default_model: Some(DEFAULT_OPENAI_MODEL),
            requires_api_key: true,
            auth_notes: None,
        },
        "openrouter" => ProviderMetadata {
            label: Some("OpenRouter"),
            default_base_url: Some("https://openrouter.ai/api/v1"),
            default_model: Some("openai/gpt-4o-mini"),
            requires_api_key: true,
            auth_notes: None,
        },
        "xai" => ProviderMetadata {
            label: Some("xAI"),
            default_base_url: Some("https://api.x.ai/v1"),
            default_model: Some("grok-3-mini"),
            requires_api_key: true,
            auth_notes: None,
        },
        _ => return None,
    };
    Some(metadata)
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

fn value_as_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(value_to_u64)
}

fn value_to_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|item| u64::try_from(item).ok()))
}
