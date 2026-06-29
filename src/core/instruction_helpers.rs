fn parse_frontmatter(text: &str, path: &Path) -> Result<ParsedFrontmatter, String> {
    let lines = text.lines().collect::<Vec<_>>();
    if lines.first().map(|line| line.trim()) != Some("---") {
        return Err(format!(
            "Skill file missing YAML frontmatter: {}",
            path.display()
        ));
    }
    let Some(closing_index) = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.trim() == "---").then_some(index))
    else {
        return Err(format!(
            "Skill file has unterminated YAML frontmatter: {}",
            path.display()
        ));
    };
    let frontmatter_text = lines[1..closing_index].join("\n");
    let body = lines[closing_index + 1..].join("\n");
    let data = serde_yaml::from_str::<serde_yaml::Value>(&frontmatter_text).map_err(|error| {
        format!(
            "Failed to parse skill frontmatter: {}: {error}",
            path.display()
        )
    })?;
    let serde_yaml::Value::Mapping(mapping) = data else {
        return Err(format!(
            "Skill frontmatter must be a YAML object: {}",
            path.display()
        ));
    };
    let mut normalized = BTreeMap::new();
    for (key, value) in mapping {
        let key = match key {
            serde_yaml::Value::String(key) => key,
            other => serde_yaml::to_string(&other)
                .unwrap_or_default()
                .trim()
                .to_string(),
        };
        let value = serde_json::to_value(value).map_err(|error| error.to_string())?;
        normalized.insert(key, value);
    }
    Ok(ParsedFrontmatter {
        data: normalized,
        content: body,
    })
}

struct ParsedFrontmatter {
    data: BTreeMap<String, Value>,
    content: String,
}

fn iter_pattern_matches(base_dir: &Path, seen: &mut BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for parts in [
        [".openagent", "skill"],
        [".openagent", "skills"],
        [".opencode", "skill"],
        [".opencode", "skills"],
        [".claude", "skills"],
    ] {
        let candidate = base_dir.join(parts[0]).join(parts[1]);
        if !candidate.is_dir() {
            continue;
        }
        for path in recursive_skill_files(&candidate) {
            if seen.insert(path.clone()) {
                result.push(path);
            }
        }
    }
    result
}

fn recursive_skill_files(root: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if root.is_file() {
        if root.file_name().and_then(OsStr::to_str) == Some("SKILL.md") {
            result.push(canonicalize_existing(root));
        }
        return result;
    }
    let mut entries = read_dir_paths(root);
    entries.sort();
    for entry in entries {
        if entry.is_dir() {
            result.extend(recursive_skill_files(&entry));
        } else if entry.file_name().and_then(OsStr::to_str) == Some("SKILL.md") {
            result.push(canonicalize_existing(&entry));
        }
    }
    result
}
