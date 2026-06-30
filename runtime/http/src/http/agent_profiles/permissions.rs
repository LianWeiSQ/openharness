fn runtime_is_subagent_mode(mode: &str) -> bool {
    matches!(mode, "subagent" | "all")
}

fn runtime_profile_permission_ruleset_value(value: &Value) -> Option<&str> {
    let permission = value.get("permission")?;
    if let Some(raw) = permission.as_str() {
        return Some(raw);
    }
    permission
        .get("ruleset")
        .or_else(|| permission.get("default"))
        .or_else(|| permission.get("mode"))
        .and_then(Value::as_str)
}

fn runtime_profile_task_permissions(value: &Value) -> Vec<TaskPermissionRule> {
    let Some(task) = value
        .get("permission")
        .and_then(|permission| permission.get("task"))
        .or_else(|| value.get("task_permissions"))
        .or_else(|| value.get("task_permission"))
    else {
        return Vec::new();
    };
    runtime_parse_task_permission_rules(task)
}

fn runtime_parse_task_permission_rules(value: &Value) -> Vec<TaskPermissionRule> {
    if let Some(object) = value.as_object() {
        return object
            .iter()
            .filter_map(|(pattern, action)| {
                runtime_task_permission_action(action).map(|action| TaskPermissionRule {
                    pattern: pattern.clone(),
                    action,
                })
            })
            .collect();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let pattern = item
                .get("pattern")
                .or_else(|| item.get("subagent"))
                .or_else(|| item.get("agent"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let action = item
                .get("action")
                .and_then(runtime_task_permission_action)?;
            Some(TaskPermissionRule {
                pattern: pattern.to_string(),
                action,
            })
        })
        .collect()
}

fn runtime_task_permission_action(value: &Value) -> Option<PermissionAction> {
    let raw = value.as_str()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "allow" | "allowed" => Some(PermissionAction::Allow),
        "deny" | "denied" => Some(PermissionAction::Deny),
        "ask" | "prompt" => Some(PermissionAction::Ask),
        _ => None,
    }
}

fn runtime_permission_manager_for_agent(
    ruleset: PermissionRuleset,
    agent_profile: Option<&RuntimeSubagentProfile>,
) -> PermissionManager {
    let mut manager = PermissionManager::new();
    manager.set_ruleset(ruleset);
    if let Some(profile) = agent_profile {
        for rule in &profile.task_permissions {
            manager.add_rule(permission_rule(
                TASK_TOOL_ID,
                rule.action.clone(),
                Some(&rule.pattern),
            ));
        }
    }
    manager
}
