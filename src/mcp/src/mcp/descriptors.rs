#[must_use]
pub fn build_tool_descriptors_from_values(
    server: &RemoteMcpServerConfig,
    tools: &[Value],
) -> Vec<RemoteMcpToolDescriptor> {
    let mut descriptors = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for tool in tools {
        let original_name = tool
            .get("name")
            .filter(|value| legacy_truthy(value))
            .map_or_else(String::new, value_to_legacy_string)
            .trim()
            .to_string();
        if original_name.is_empty() || !tool_allowed(&original_name, &server.tools) {
            continue;
        }

        let mut dynamic_name = dynamic_tool_name(&server.name, &original_name);
        if seen.contains(&dynamic_name) {
            dynamic_name = format!(
                "{}_{}",
                dynamic_name,
                duplicate_suffix(&server.name, &original_name)
            );
        }
        seen.insert(dynamic_name.clone());

        let title = tool
            .get("title")
            .filter(|value| legacy_truthy(value))
            .map_or_else(|| original_name.clone(), value_to_legacy_string);
        let raw_description = tool
            .get("description")
            .filter(|value| legacy_truthy(value))
            .map_or_else(String::new, value_to_legacy_string);
        let description = tool_description(&server.name, &original_name, &raw_description);
        let input_schema = normalize_input_schema(tool.get("inputSchema"));
        let annotations_safe = tool
            .get("annotations")
            .filter(|value| legacy_truthy(value))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let annotations = if annotations_safe.is_object() {
            annotations_safe.clone()
        } else {
            json!({})
        };
        let execution = tool.get("execution").cloned().unwrap_or(Value::Null);
        let raw_metadata = json!({
            "title": title,
            "description": raw_description,
            "annotations": annotations_safe,
            "execution": execution,
        });

        descriptors.push(RemoteMcpToolDescriptor {
            server_name: server.name.clone(),
            original_name,
            dynamic_name,
            title,
            description,
            input_schema,
            annotations,
            raw_metadata,
        });
    }

    descriptors
}

#[must_use]
pub fn tool_allowed(tool_name: &str, filters: &McpToolFilter) -> bool {
    if !filters.allow.is_empty()
        && !filters
            .allow
            .iter()
            .any(|pattern| fnmatchcase(tool_name, pattern))
    {
        return false;
    }
    if filters
        .deny
        .iter()
        .any(|pattern| fnmatchcase(tool_name, pattern))
    {
        return false;
    }
    true
}
