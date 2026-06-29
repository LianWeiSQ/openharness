use super::*;

#[derive(Clone, Debug, Default)]
pub(crate) struct RunAgentProfile {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) mode: String,
    pub(super) model: Option<String>,
    pub(super) provider: Option<String>,
    pub(super) permission: Option<String>,
    pub(super) prompt: Option<String>,
    pub(super) tools: Vec<String>,
    pub(super) max_steps: Option<u64>,
    pub(super) hidden: bool,
    pub(super) source_path: Option<PathBuf>,
    pub(super) loaded: bool,
}

const BUILD_AGENT_PROMPT: &str = include_str!("../../../skill/prompts/build.txt");
const EXPLORE_AGENT_PROMPT: &str = include_str!("../../../skill/prompts/explore.txt");
const PLAN_AGENT_PROMPT: &str = include_str!("../../../skill/prompts/plan.txt");

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

pub(super) fn load_agent_profile_from_args(
    args: &[String],
    _workspace: &Path,
) -> Result<Option<RunAgentProfile>, String> {
    let Some(raw_name) = value_for(args, &["--agent"]) else {
        return Ok(None);
    };
    Ok(Some(load_agent_profile_by_name(args, &raw_name)?))
}

pub(crate) fn load_agent_profile_by_name(
    args: &[String],
    raw_name: &str,
) -> Result<RunAgentProfile, String> {
    let agent_id = sanitize_identifier(raw_name);
    let path = agent_registry_dir(args).join(format!("{agent_id}.json"));
    let value = read_json_file(&path);
    if value.as_object().is_some_and(|object| !object.is_empty()) {
        return agent_profile_from_value(&value, Some(path), &agent_id, raw_name, true);
    }
    available_agent_profiles(args)
        .into_iter()
        .find(|profile| {
            profile.id == agent_id
                || sanitize_identifier(&profile.name) == agent_id
                || profile.name.eq_ignore_ascii_case(raw_name)
        })
        .ok_or_else(|| format!("agent profile not found: {raw_name} ({})", path.display()))
}

pub(crate) fn available_agent_profiles(args: &[String]) -> Vec<RunAgentProfile> {
    let mut profiles = builtin_agent_profiles()
        .into_iter()
        .map(|profile| (profile.id.clone(), profile))
        .collect::<BTreeMap<_, _>>();
    let dir = agent_registry_dir(args);
    let mut paths = fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let value = read_json_file(&path);
        if value.as_object().is_none_or(Map::is_empty) {
            continue;
        }
        let fallback_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(sanitize_identifier)
            .unwrap_or_else(|| "agent".to_string());
        if let Ok(profile) =
            agent_profile_from_value(&value, Some(path), &fallback_id, &fallback_id, true)
        {
            profiles.insert(profile.id.clone(), profile);
        }
    }
    profiles.into_values().collect()
}

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

pub(super) fn task_subagent_descriptors(args: &[String]) -> Vec<TaskSubagentDescriptor> {
    available_subagent_profiles(args, false)
        .into_iter()
        .map(|profile| TaskSubagentDescriptor {
            id: profile.id,
            name: profile.name,
            description: profile.description.unwrap_or_default(),
        })
        .collect()
}

fn agent_profile_from_value(
    value: &Value,
    source_path: Option<PathBuf>,
    fallback_id: &str,
    fallback_name: &str,
    loaded: bool,
) -> Result<RunAgentProfile, String> {
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("primary")
        .to_ascii_lowercase();
    if !matches!(mode.as_str(), "primary" | "subagent" | "all") {
        return Err(format!(
            "agent profile {fallback_id} has invalid mode '{mode}'; expected primary, subagent, or all"
        ));
    }
    Ok(RunAgentProfile {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .map(sanitize_identifier)
            .unwrap_or_else(|| sanitize_identifier(fallback_id)),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(fallback_name)
            .to_string(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        mode,
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        provider: value
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string),
        permission: value
            .get("permission")
            .and_then(Value::as_str)
            .map(str::to_string),
        prompt: value
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::to_string),
        tools: value
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        max_steps: value
            .get("max_steps")
            .or_else(|| value.get("steps"))
            .and_then(Value::as_u64),
        hidden: value
            .get("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        source_path,
        loaded,
    })
}

fn builtin_agent_profiles() -> Vec<RunAgentProfile> {
    vec![
        builtin_agent_profile(
            "build",
            "Build",
            "Primary implementation agent for coding, testing, and general project work.",
            "primary",
            Some("PLAN_ONLY"),
            BUILD_AGENT_PROMPT,
            &[],
        ),
        builtin_agent_profile(
            "general",
            "General",
            "General-purpose subagent for complex multi-step implementation, debugging, and research tasks.",
            "subagent",
            Some("PLAN_ONLY"),
            BUILD_AGENT_PROMPT,
            &[],
        ),
        builtin_agent_profile(
            "explore",
            "Explore",
            "Read-only code exploration subagent for fast search, mapping, and evidence gathering.",
            "subagent",
            Some("READONLY"),
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
        builtin_agent_profile(
            "plan",
            "Plan",
            "Planning subagent for architecture analysis, implementation strategy, and task breakdowns.",
            "subagent",
            Some("PLAN_ONLY"),
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

fn builtin_agent_profile(
    id: &str,
    name: &str,
    description: &str,
    mode: &str,
    permission: Option<&str>,
    prompt: &str,
    tools: &[&str],
) -> RunAgentProfile {
    RunAgentProfile {
        id: id.to_string(),
        name: name.to_string(),
        description: Some(description.to_string()),
        mode: mode.to_string(),
        model: None,
        provider: None,
        permission: permission.map(str::to_string),
        prompt: Some(prompt.trim_start_matches('\u{feff}').to_string()),
        tools: tools.iter().map(|item| (*item).to_string()).collect(),
        max_steps: None,
        hidden: false,
        source_path: None,
        loaded: true,
    }
}

pub(crate) fn agent_profile_public_value(profile: &RunAgentProfile) -> Value {
    json!({
        "id": profile.id.clone(),
        "name": profile.name.clone(),
        "description": profile.description.clone(),
        "mode": profile.mode.clone(),
        "model": profile.model.clone(),
        "provider": profile.provider.clone(),
        "permission": profile.permission.clone(),
        "tools": profile.tools.clone(),
        "max_steps": profile.max_steps,
        "hidden": profile.hidden,
        "loaded": profile.loaded,
        "source_path": profile.source_path.as_ref().map(|path| path.to_string_lossy().to_string()),
    })
}

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

pub(super) fn permission_ruleset_from_args(
    args: &[String],
    agent_profile: Option<&RunAgentProfile>,
) -> Result<PermissionRuleset, String> {
    let raw = value_for(args, &["--permission"])
        .or_else(|| agent_profile.and_then(|profile| profile.permission.clone()))
        .unwrap_or_else(|| "PLAN_ONLY".to_string());
    parse_permission_ruleset(&raw)
}

pub(super) fn permission_ruleset_for_profile(
    agent_profile: &RunAgentProfile,
    fallback: PermissionRuleset,
) -> Result<PermissionRuleset, String> {
    agent_profile
        .permission
        .as_deref()
        .map(parse_permission_ruleset)
        .transpose()
        .map(|value| value.unwrap_or(fallback))
}

pub(super) fn is_subagent_mode(mode: &str) -> bool {
    matches!(mode, "subagent" | "all")
}

pub(super) fn parse_permission_ruleset(raw: &str) -> Result<PermissionRuleset, String> {
    match raw.trim().to_ascii_uppercase().replace('-', "_").as_str() {
        "FULL" | "ALLOW" | "AUTO" => Ok(PermissionRuleset::Full),
        "READONLY" | "READ_ONLY" => Ok(PermissionRuleset::Readonly),
        "PLAN_ONLY" | "ASK" => Ok(PermissionRuleset::PlanOnly),
        "NONE" | "DENY" => Ok(PermissionRuleset::None),
        _ => Err("permission must be FULL, READONLY, PLAN_ONLY, or NONE".to_string()),
    }
}
