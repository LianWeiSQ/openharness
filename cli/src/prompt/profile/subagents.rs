pub(super) fn available_subagent_profiles(
    args: &[String],
    include_hidden: bool,
) -> Vec<RunAgentProfile> {
    available_agent_profiles(args)
        .into_iter()
        .filter(|profile| is_subagent_mode(&profile.mode))
        .filter(|profile| include_hidden || !profile.hidden)
        .collect()
}

pub(super) fn task_subagent_descriptors(
    args: &[String],
    agent_profile: Option<&RunAgentProfile>,
    parent_session: Option<&Session>,
) -> Vec<TaskSubagentDescriptor> {
    available_subagent_profiles(args, false)
        .into_iter()
        .filter(|profile| {
            agent_profile.is_none_or(|parent| {
                task_subagent_is_visible(&parent.task_permissions, &profile.id)
            })
        })
        .filter(|profile| {
            parent_session
                .is_none_or(|session| subagent_task_governance_error(session, profile).is_none())
        })
        .map(|profile| TaskSubagentDescriptor {
            id: profile.id,
            name: profile.name,
            description: profile.description.unwrap_or_default(),
        })
        .collect()
}

pub(super) fn max_subagent_depth_cli() -> u64 {
    std::env::var("OPENAGENT_MAX_SUBAGENT_DEPTH")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(3)
        .max(1)
}

pub(super) fn child_task_depth(parent_session: &Session) -> u64 {
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

pub(super) fn task_root_session_id(parent_session: &Session) -> String {
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

pub(super) fn parent_task_lineage(parent_session: &Session) -> Vec<String> {
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

pub(super) fn child_task_lineage(parent_session: &Session, child_agent: &str) -> Vec<String> {
    let mut lineage = parent_task_lineage(parent_session);
    lineage.push(child_agent.to_string());
    lineage
}

pub(super) fn subagent_task_governance_error(
    parent_session: &Session,
    profile: &RunAgentProfile,
) -> Option<String> {
    let lineage = parent_task_lineage(parent_session);
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
    let next_depth = child_task_depth(parent_session);
    let max_depth = max_subagent_depth_cli();
    if next_depth > max_depth {
        return Some(format!(
            "subagent nesting depth {next_depth} exceeds max subagent depth {max_depth}"
        ));
    }
    None
}
