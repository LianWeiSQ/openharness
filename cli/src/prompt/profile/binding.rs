pub(super) fn bind_agent_profile_system_prompt(
    session: &mut Session,
    store: &FileSessionStore,
    run_id: &str,
    profile: Option<&RunAgentProfile>,
) -> Result<(), String> {
    let Some(profile) = profile else {
        return Ok(());
    };
    let Some(prompt) = profile
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let already_bound = session.messages.iter().any(|message| {
        message.role == Role::System
            && message
                .metadata
                .get("agent_profile")
                .and_then(Value::as_str)
                == Some(profile.id.as_str())
    });
    if already_bound {
        return Ok(());
    }
    let mut message = chat_message(Role::System, prompt.to_string());
    message
        .metadata
        .insert("agent_profile".to_string(), json!(profile.id.clone()));
    message
        .metadata
        .insert("agent_mode".to_string(), json!(profile.mode.clone()));
    let index = session.messages.len() as u64;
    session.add(message.clone());
    store
        .append_message(session, &message, run_id, index)
        .map_err(|error| format!("failed to record agent system prompt: {error}"))
}

pub(super) fn filter_tools_for_agent(
    tools: Vec<ToolSchema>,
    agent_profile: Option<&RunAgentProfile>,
) -> Vec<ToolSchema> {
    let Some(profile) = agent_profile else {
        return tools;
    };
    if profile.tools.is_empty() {
        return tools;
    }
    tools
        .into_iter()
        .filter(|tool| {
            profile
                .tools
                .iter()
                .any(|pattern| wildcard_match(pattern, &tool.name))
        })
        .collect()
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" || pattern == value {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return value.ends_with(suffix);
    }
    if let Some((prefix, suffix)) = pattern.split_once('*') {
        return value.starts_with(prefix) && value.ends_with(suffix);
    }
    false
}
