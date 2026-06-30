fn runtime_subagent_profile(id: &str, workspace: &Path) -> Option<RuntimeSubagentProfile> {
    let normalized = sanitize_runtime_agent_id(id);
    runtime_subagent_profiles(workspace)
        .into_iter()
        .find(|profile| profile.id == normalized || profile.name.eq_ignore_ascii_case(id))
}

fn runtime_agent_profile(id: &str, workspace: &Path) -> Option<RuntimeSubagentProfile> {
    let normalized = sanitize_runtime_agent_id(id);
    runtime_agent_profiles(workspace)
        .into_iter()
        .find(|profile| profile.id == normalized || profile.name.eq_ignore_ascii_case(id))
}

fn runtime_agent_profile_for_session(session: &Session) -> Option<RuntimeSubagentProfile> {
    if let Some(profile_value) = session.metadata.get("agent_profile") {
        let fallback_id = profile_value
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| session.metadata.get("agent").and_then(Value::as_str))
            .unwrap_or("agent");
        if let Some(profile) = runtime_agent_profile_from_value(profile_value, fallback_id, None) {
            return Some(profile);
        }
    }
    session
        .metadata
        .get("agent")
        .and_then(Value::as_str)
        .and_then(|id| runtime_agent_profile(id, &session.directory))
}
