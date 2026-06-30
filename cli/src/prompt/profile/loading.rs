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
    for path in agent_profile_path_candidates(args, &agent_id) {
        if let Some(profile) = load_agent_profile_from_path(&path, &agent_id, raw_name)? {
            if profile.disabled {
                return Err(format!("agent profile {raw_name} is disabled"));
            }
            return Ok(profile);
        }
    }
    available_agent_profiles(args)
        .into_iter()
        .find(|profile| {
            profile.id == agent_id
                || sanitize_identifier(&profile.name) == agent_id
                || profile.name.eq_ignore_ascii_case(raw_name)
        })
        .ok_or_else(|| format!("agent profile not found: {raw_name}"))
}

pub(crate) fn available_agent_profiles(args: &[String]) -> Vec<RunAgentProfile> {
    let mut profiles = builtin_agent_profiles()
        .into_iter()
        .map(|profile| (profile.id.clone(), profile))
        .collect::<BTreeMap<_, _>>();
    let mut paths = agent_registry_dirs(args)
        .into_iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| agent_profile_file_kind(path).is_some())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let fallback_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(sanitize_identifier)
            .unwrap_or_else(|| "agent".to_string());
        if let Ok(Some(profile)) = load_agent_profile_from_path(&path, &fallback_id, &fallback_id)
            && !profile.disabled
        {
            profiles.insert(profile.id.clone(), profile);
        }
    }
    profiles.into_values().collect()
}

fn agent_registry_dirs(args: &[String]) -> Vec<PathBuf> {
    let workspace = workspace_from_args(args);
    vec![
        agent_registry_dir(args),
        workspace.join(".opencode/agents"),
        workspace.join(".opencode/agent"),
    ]
}

fn agent_profile_path_candidates(args: &[String], agent_id: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for dir in agent_registry_dirs(args) {
        for extension in ["json", "md", "markdown"] {
            paths.push(dir.join(format!("{agent_id}.{extension}")));
        }
    }
    paths
}

fn agent_profile_file_kind(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("json") => Some("json"),
        Some("md" | "markdown") => Some("markdown"),
        _ => None,
    }
}

fn load_agent_profile_from_path(
    path: &Path,
    fallback_id: &str,
    fallback_name: &str,
) -> Result<Option<RunAgentProfile>, String> {
    let Some(kind) = agent_profile_file_kind(path) else {
        return Ok(None);
    };
    let value = if kind == "json" {
        read_json_file(path)
    } else {
        match markdown_agent_profile_value(path) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        }
    };
    if value.as_object().is_none_or(Map::is_empty) {
        return Ok(None);
    }
    agent_profile_from_value(
        &value,
        Some(path.to_path_buf()),
        fallback_id,
        fallback_name,
        true,
    )
    .map(Some)
}

fn markdown_agent_profile_value(path: &Path) -> Result<Value, String> {
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
