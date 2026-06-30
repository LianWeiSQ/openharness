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
