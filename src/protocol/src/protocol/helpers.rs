fn readonly_tools() -> impl Iterator<Item = &'static str> {
    [
        "read", "glob", "grep", "ls", "skill", "todoread", "question",
    ]
    .into_iter()
}

fn plan_only_tools() -> impl Iterator<Item = &'static str> {
    [
        "read",
        "glob",
        "grep",
        "ls",
        "skill",
        "todoread",
        "todowrite",
        "question",
    ]
    .into_iter()
}

fn permission_rule(tool: &str, action: PermissionAction) -> PermissionRule {
    PermissionRule {
        tool: tool.to_string(),
        action,
        pattern: Some("*".to_string()),
        condition: None,
    }
}

fn provider_options(options: Option<&BTreeMap<String, Value>>) -> BTreeMap<String, Value> {
    let runtime_keys = RUNTIME_OPTION_KEYS.iter().copied().collect::<BTreeSet<_>>();
    options
        .into_iter()
        .flat_map(BTreeMap::iter)
        .filter(|(key, _value)| !runtime_keys.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn append_text_section(lines: &mut Vec<String>, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(format!("{title}:"));
    lines.extend(items.iter().map(|item| format!("- {item}")));
}

fn append_files_section(lines: &mut Vec<String>, files: &[WorkStateFile]) {
    if files.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Files:".to_string());
    lines.extend(
        files
            .iter()
            .map(|file| format!("- {} ({}) - {}", file.path, file.status, file.note)),
    );
}

fn json_object(items: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(Map::from_iter(
        items
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    ))
}

fn stable_json_dumps(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => {
            if *value {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("string serializes"),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(stable_json_dumps)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        Value::Object(items) => {
            let inner = items
                .iter()
                .map(|(key, value)| {
                    let key = serde_json::to_string(key).expect("key serializes");
                    let value = stable_json_dumps(value);
                    format!("{key}: {value}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{inner}}}")
        }
    }
}
