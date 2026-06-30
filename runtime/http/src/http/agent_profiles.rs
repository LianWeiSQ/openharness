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

#[derive(Clone, Debug)]
struct RuntimeSubagentProfile {
    id: String,
    name: String,
    description: String,
    mode: String,
    permission: PermissionRuleset,
    task_permissions: Vec<TaskPermissionRule>,
    prompt: String,
    tools: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    max_steps: Option<u64>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    color: Option<String>,
    disabled: bool,
    model_options: BTreeMap<String, Value>,
    hidden: bool,
    source_path: Option<PathBuf>,
}

fn runtime_subagent_profiles(workspace: &Path) -> Vec<RuntimeSubagentProfile> {
    runtime_agent_profiles(workspace)
        .into_iter()
        .filter(|profile| runtime_is_subagent_mode(&profile.mode))
        .collect()
}

fn runtime_agent_profiles(workspace: &Path) -> Vec<RuntimeSubagentProfile> {
    let mut profiles = builtin_runtime_subagent_profiles()
        .into_iter()
        .map(|profile| (profile.id.clone(), profile))
        .collect::<BTreeMap<_, _>>();
    let mut paths = runtime_agent_registry_dirs(workspace)
        .into_iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| runtime_agent_profile_file_kind(path).is_some())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let fallback_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(sanitize_runtime_agent_id)
            .unwrap_or_else(|| "agent".to_string());
        if let Some(profile) = runtime_agent_profile_from_path(&path, &fallback_id)
            && !profile.disabled
        {
            profiles.insert(profile.id.clone(), profile);
        }
    }
    profiles.into_values().collect()
}

fn runtime_agent_registry_dirs(workspace: &Path) -> Vec<PathBuf> {
    vec![
        workspace.join(".openagent/agents"),
        workspace.join(".opencode/agents"),
        workspace.join(".opencode/agent"),
    ]
}

fn runtime_agent_profile_file_kind(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("json") => Some("json"),
        Some("md" | "markdown") => Some("markdown"),
        _ => None,
    }
}

fn runtime_agent_profile_from_path(
    path: &Path,
    fallback_id: &str,
) -> Option<RuntimeSubagentProfile> {
    let kind = runtime_agent_profile_file_kind(path)?;
    let value = if kind == "json" {
        read_json_file(path)
    } else {
        markdown_runtime_agent_profile_value(path).ok()?
    };
    runtime_agent_profile_from_value(&value, fallback_id, Some(path.to_path_buf()))
}

fn markdown_runtime_agent_profile_value(path: &Path) -> Result<Value, String> {
    let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut value = json!({});
    let mut body = raw.as_str();
    if let Some(rest) = raw.trim_start_matches('\u{feff}').strip_prefix("---")
        && let Some((frontmatter, tail)) = rest.split_once("---")
    {
        value = serde_yaml::from_str::<Value>(frontmatter).unwrap_or_else(|_| json!({}));
        body = tail.trim_start_matches('\n');
    }
    if value.as_object().is_none() {
        value = json!({});
    }
    if let Some(object) = value.as_object_mut() {
        let prompt = body.trim_start_matches('\n').trim_end();
        if !prompt.trim().is_empty() && !object.contains_key("prompt") {
            object.insert("prompt".to_string(), json!(prompt));
        }
    }
    Ok(value)
}

fn builtin_runtime_subagent_profiles() -> Vec<RuntimeSubagentProfile> {
    vec![
        builtin_runtime_subagent_profile(
            "coder",
            "Coder",
            "Implementation-focused profile",
            PermissionRuleset::PlanOnly,
            BUILD_AGENT_PROMPT,
            &[],
        ),
        builtin_runtime_subagent_profile(
            "reviewer",
            "Reviewer",
            "Review and risk-focused profile",
            PermissionRuleset::Readonly,
            REVIEW_AGENT_PROMPT,
            &[
                "read",
                "glob",
                "grep",
                "ls",
                "code_search",
                "skill",
                "todoread",
            ],
        ),
        builtin_runtime_subagent_profile(
            "planner",
            "Planner",
            "Plan-first profile for large changes",
            PermissionRuleset::PlanOnly,
            PLAN_AGENT_PROMPT,
            &[
                "read",
                "glob",
                "grep",
                "ls",
                "code_search",
                "skill",
                "todoread",
                "todowrite",
                "question",
            ],
        ),
        builtin_runtime_subagent_profile(
            "general",
            "General",
            "General-purpose subagent for complex multi-step tasks",
            PermissionRuleset::PlanOnly,
            BUILD_AGENT_PROMPT,
            &[],
        ),
        builtin_runtime_subagent_profile(
            "explore",
            "Explore",
            "Read-only code exploration subagent",
            PermissionRuleset::Readonly,
            EXPLORE_AGENT_PROMPT,
            &[
                "read",
                "glob",
                "grep",
                "ls",
                "code_search",
                "skill",
                "todoread",
            ],
        ),
        builtin_runtime_subagent_profile(
            "plan",
            "Plan",
            "Planning subagent for architecture and task breakdowns",
            PermissionRuleset::PlanOnly,
            PLAN_AGENT_PROMPT,
            &[
                "read",
                "glob",
                "grep",
                "ls",
                "code_search",
                "skill",
                "todoread",
                "todowrite",
                "question",
            ],
        ),
    ]
}

fn builtin_runtime_subagent_profile(
    id: &str,
    name: &str,
    description: &str,
    permission: PermissionRuleset,
    prompt: &str,
    tools: &[&str],
) -> RuntimeSubagentProfile {
    RuntimeSubagentProfile {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        mode: "subagent".to_string(),
        permission,
        task_permissions: Vec::new(),
        prompt: prompt.trim_start_matches('\u{feff}').to_string(),
        tools: tools.iter().map(|item| (*item).to_string()).collect(),
        provider: None,
        model: None,
        max_steps: None,
        temperature: None,
        top_p: None,
        color: None,
        disabled: false,
        model_options: BTreeMap::new(),
        hidden: false,
        source_path: None,
    }
}

fn runtime_agent_profile_from_value(
    value: &Value,
    fallback_id: &str,
    source_path: Option<PathBuf>,
) -> Option<RuntimeSubagentProfile> {
    if value.as_object().is_none_or(Map::is_empty) {
        return None;
    }
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("primary")
        .trim()
        .to_ascii_lowercase();
    if !matches!(mode.as_str(), "primary" | "subagent" | "all") {
        return None;
    }
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(sanitize_runtime_agent_id)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| sanitize_runtime_agent_id(fallback_id));
    let permission = runtime_profile_permission_ruleset_value(value)
        .and_then(|raw| parse_permission_ruleset(raw).ok())
        .unwrap_or(PermissionRuleset::PlanOnly);
    Some(RuntimeSubagentProfile {
        id: if id.is_empty() {
            "agent".to_string()
        } else {
            id
        },
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(fallback_id)
            .to_string(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        mode,
        permission,
        task_permissions: runtime_profile_task_permissions(value),
        prompt: value
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or(BUILD_AGENT_PROMPT)
            .trim_start_matches('\u{feff}')
            .to_string(),
        tools: runtime_profile_string_list(value.get("tools")),
        provider: value
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string),
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        max_steps: value
            .get("max_steps")
            .or_else(|| value.get("steps"))
            .or_else(|| value.get("maxSteps"))
            .and_then(Value::as_u64),
        temperature: value.get("temperature").and_then(Value::as_f64),
        top_p: value
            .get("top_p")
            .or_else(|| value.get("topP"))
            .and_then(Value::as_f64),
        color: value
            .get("color")
            .and_then(Value::as_str)
            .map(str::to_string),
        disabled: value
            .get("disabled")
            .or_else(|| value.get("disable"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        model_options: runtime_profile_model_options(value),
        hidden: value
            .get("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        source_path,
    })
}

fn runtime_profile_string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        Some(Value::String(item)) => item
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn runtime_profile_model_options(value: &Value) -> BTreeMap<String, Value> {
    const KNOWN_KEYS: &[&str] = &[
        "id",
        "name",
        "description",
        "mode",
        "model",
        "provider",
        "permission",
        "task",
        "task_permissions",
        "tools",
        "prompt",
        "steps",
        "max_steps",
        "maxSteps",
        "hidden",
        "color",
        "disabled",
        "disable",
        "temperature",
        "top_p",
        "topP",
        "model_options",
        "options",
    ];
    let mut options = BTreeMap::new();
    if let Some(object) = value.as_object() {
        for (key, item) in object {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                options.insert(key.clone(), item.clone());
            }
        }
    }
    if let Some(temperature) = value.get("temperature").and_then(Value::as_f64) {
        options.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = value
        .get("top_p")
        .or_else(|| value.get("topP"))
        .and_then(Value::as_f64)
    {
        options.insert("top_p".to_string(), json!(top_p));
    }
    for key in ["model_options", "options"] {
        if let Some(object) = value.get(key).and_then(Value::as_object) {
            for (option_key, option_value) in object {
                options.insert(option_key.clone(), option_value.clone());
            }
        }
    }
    options
}

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

fn sanitize_runtime_agent_id(value: &str) -> String {
    let mut output = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    while output.contains("--") {
        output = output.replace("--", "-");
    }
    if output.is_empty() {
        "agent".to_string()
    } else {
        output
    }
}
