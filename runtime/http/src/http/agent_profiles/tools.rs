fn runtime_task_subagent_descriptors(
    workspace: &Path,
    agent_profile: Option<&RuntimeSubagentProfile>,
    parent_session: Option<&Session>,
) -> Vec<TaskSubagentDescriptor> {
    runtime_subagent_profiles(workspace)
        .into_iter()
        .filter(|profile| !profile.hidden)
        .filter(|profile| {
            agent_profile.is_none_or(|parent| {
                task_subagent_is_visible(&parent.task_permissions, &profile.id)
            })
        })
        .filter(|profile| {
            parent_session
                .is_none_or(|session| runtime_task_governance_error(session, profile).is_none())
        })
        .map(|profile| TaskSubagentDescriptor {
            id: profile.id,
            name: profile.name,
            description: profile.description,
        })
        .collect()
}

fn filter_runtime_tools_for_profile(
    tools: Vec<ToolSchema>,
    profile: Option<&RuntimeSubagentProfile>,
) -> Vec<ToolSchema> {
    let Some(profile) = profile else {
        return tools;
    };
    if profile.tools.is_empty() {
        return tools;
    }
    tools
        .into_iter()
        .filter(|tool| runtime_tool_allowed_for_profile(&tool.name, profile))
        .collect()
}

fn runtime_tool_allowed_for_profile(tool_name: &str, profile: &RuntimeSubagentProfile) -> bool {
    profile
        .tools
        .iter()
        .any(|pattern| runtime_tool_pattern_matches(pattern, tool_name))
}

fn runtime_tool_pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" || pattern == value {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    false
}

fn toolkit_with_runtime_task_tool(
    session: &Session,
    agent_profile: Option<&RuntimeSubagentProfile>,
) -> Toolkit {
    let mut toolkit = Toolkit::with_builtins();
    register_task_tool(
        &mut toolkit.registry,
        &runtime_task_subagent_descriptors(&session.directory, agent_profile, Some(session)),
    );
    toolkit
}
