pub(super) fn provider_and_model_from_args(
    args: &[String],
    agent_profile: Option<&RunAgentProfile>,
) -> (String, String) {
    if let Some(raw) = value_for(args, &["--model", "-m"])
        && let Some((provider, model)) = raw.split_once('/')
        && !provider.is_empty()
        && !model.is_empty()
    {
        let provider = normalize_provider(Some(provider)).unwrap_or_else(|_| provider.to_string());
        return (provider, model.to_string());
    }
    if value_for(args, &["--model", "-m"]).is_none()
        && let Some(raw) = agent_profile.and_then(|profile| profile.model.as_deref())
        && let Some((provider, model)) = raw.split_once('/')
        && !provider.is_empty()
        && !model.is_empty()
    {
        let provider = normalize_provider(Some(provider)).unwrap_or_else(|_| provider.to_string());
        return (provider, model.to_string());
    }
    let provider = value_for(args, &["--provider"])
        .or_else(|| agent_profile.and_then(|profile| profile.provider.clone()))
        .unwrap_or_else(active_provider);
    let provider = normalize_provider(Some(&provider)).unwrap_or(provider);
    let model = value_for(args, &["--model", "-m"])
        .or_else(|| agent_profile.and_then(|profile| profile.model.clone()))
        .or_else(|| provider_env_value(&provider, "model"))
        .unwrap_or_else(|| default_model_for_provider(&provider));
    (provider, model)
}

pub(super) fn provider_and_model_for_subagent(
    parent_provider: &str,
    parent_model: &str,
    agent_profile: &RunAgentProfile,
) -> (String, String) {
    if let Some(raw) = agent_profile.model.as_deref()
        && let Some((provider, model)) = raw.split_once('/')
        && !provider.is_empty()
        && !model.is_empty()
    {
        let provider = normalize_provider(Some(provider)).unwrap_or_else(|_| provider.to_string());
        return (provider, model.to_string());
    }
    let provider = agent_profile
        .provider
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| parent_provider.to_string());
    let provider = normalize_provider(Some(&provider)).unwrap_or(provider);
    let model = agent_profile
        .model
        .clone()
        .unwrap_or_else(|| parent_model.to_string());
    (provider, model)
}
