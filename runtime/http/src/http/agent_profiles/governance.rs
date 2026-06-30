fn max_subagent_depth() -> u64 {
    std::env::var("OPENAGENT_MAX_SUBAGENT_DEPTH")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_SUBAGENT_DEPTH)
        .max(1)
}

fn runtime_child_task_depth(parent_session: &Session) -> u64 {
    if parent_session
        .metadata
        .get("subagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        parent_session
            .metadata
            .get("task_depth")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .saturating_add(1)
    } else {
        1
    }
}

fn runtime_task_root_session_id(parent_session: &Session) -> String {
    if parent_session
        .metadata
        .get("subagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        parent_session
            .metadata
            .get("task_root_session_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(parent_session.id.as_str())
            .to_string()
    } else {
        parent_session.id.clone()
    }
}

fn runtime_parent_task_lineage(parent_session: &Session) -> Vec<String> {
    parent_session
        .metadata
        .get("task_lineage_subagents")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            parent_session
                .metadata
                .get("agent")
                .and_then(Value::as_str)
                .filter(|_| {
                    parent_session
                        .metadata
                        .get("subagent")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .map(|agent| vec![agent.to_string()])
                .unwrap_or_default()
        })
}

fn runtime_child_task_lineage(parent_session: &Session, child_agent: &str) -> Vec<String> {
    let mut lineage = runtime_parent_task_lineage(parent_session);
    lineage.push(child_agent.to_string());
    lineage
}

fn runtime_task_governance_error(
    parent_session: &Session,
    profile: &RuntimeSubagentProfile,
) -> Option<String> {
    let lineage = runtime_parent_task_lineage(parent_session);
    let parent_agent = parent_session
        .metadata
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if parent_session
        .metadata
        .get("subagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && parent_agent == profile.id
    {
        return Some(format!("subagent {} cannot call itself", profile.id));
    }
    if lineage.iter().any(|agent| agent == &profile.id) {
        return Some(format!(
            "subagent {} is already in task lineage",
            profile.id
        ));
    }
    let child_depth = runtime_child_task_depth(parent_session);
    let max_depth = max_subagent_depth();
    if child_depth > max_depth {
        return Some(format!(
            "subagent nesting depth {child_depth} exceeds max subagent depth {max_depth}"
        ));
    }
    None
}
