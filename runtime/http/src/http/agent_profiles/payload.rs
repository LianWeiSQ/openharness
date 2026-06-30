fn agents_payload(config: &HttpRuntimeConfig) -> Value {
    let mut agents = vec![json!({
        "id": "server",
        "name": "Server",
        "description": "Default server-backed coding agent",
        "mode": "primary",
        "default": true,
    })];
    agents.extend(
        runtime_subagent_profiles(&workspace(config))
            .into_iter()
            .filter(|profile| !profile.hidden)
            .map(|profile| runtime_subagent_public_value(&profile)),
    );
    json!({ "agents": agents })
}
