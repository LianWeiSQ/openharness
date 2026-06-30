pub(super) fn call_provider_for_run(
    args: &[String],
    provider: &str,
    model_id: &str,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
    agent_profile: Option<&RunAgentProfile>,
) -> Result<ProviderRunResult, String> {
    if subagent_profile_id(messages).is_some()
        && let Ok(answer) = env::var("OPENAGENT_MOCK_SUBAGENT_ANSWER")
        && !answer.is_empty()
    {
        return Ok(ProviderRunResult {
            answer,
            tool_calls: Vec::new(),
            usage: Usage::default(),
            source: "mock_subagent".to_string(),
            finish_reason: "stop".to_string(),
        });
    }
    if !messages.iter().any(|message| message.role == Role::Tool)
        && let Some(tool_calls) = mock_tool_calls_from_env()?
    {
        return Ok(ProviderRunResult {
            answer: env::var("OPENAGENT_MOCK_TOOL_PREFACE").unwrap_or_default(),
            tool_calls,
            usage: Usage::default(),
            source: "mock".to_string(),
            finish_reason: "tool_call".to_string(),
        });
    }
    if let Ok(answer) = env::var("OPENAGENT_MOCK_ANSWER")
        && !answer.is_empty()
    {
        return Ok(ProviderRunResult {
            answer,
            tool_calls: Vec::new(),
            usage: Usage::default(),
            source: "mock".to_string(),
            finish_reason: "stop".to_string(),
        });
    }
    let api_key = provider_api_key(provider, args);
    if provider_requires_api_key(provider).unwrap_or(true) && api_key.is_none() {
        return Ok(ProviderRunResult {
            answer: "hello from openagent".to_string(),
            tool_calls: Vec::new(),
            usage: Usage::default(),
            source: "offline_fallback_missing_api_key".to_string(),
            finish_reason: "stop".to_string(),
        });
    }
    let api_key = api_key.unwrap_or_default();
    if provider == "anthropic" {
        call_anthropic_provider(args, &api_key, model_id, messages, tools, stream_sink)
    } else {
        call_openai_compatible_provider(
            args,
            provider,
            &api_key,
            model_id,
            messages,
            tools,
            stream_sink,
            agent_profile,
        )
    }
}

fn subagent_profile_id(messages: &[ChatMessage]) -> Option<&str> {
    messages.iter().find_map(|message| {
        (message.role == Role::System
            && message
                .metadata
                .get("agent_mode")
                .and_then(Value::as_str)
                .is_some_and(|mode| matches!(mode, "subagent" | "all")))
        .then(|| {
            message
                .metadata
                .get("agent_profile")
                .and_then(Value::as_str)
        })
        .flatten()
    })
}
