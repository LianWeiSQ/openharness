#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn protocol_crate_name() -> &'static str {
    openagent_protocol::crate_name()
}

#[derive(Clone, Debug, Default)]
pub struct PermissionManager {
    rules: Vec<PermissionRule>,
}

impl PermissionManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_ruleset(&mut self, name: PermissionRuleset) {
        self.rules = ruleset(name).rules;
    }

    pub fn add_rule(&mut self, rule: PermissionRule) {
        self.rules.push(rule);
    }

    #[must_use]
    pub fn evaluate(&self, tool: &str, pattern: &str) -> Option<&PermissionRule> {
        let mut matched = None;
        for rule in &self.rules {
            if glob_match(&rule.tool, tool)
                && rule
                    .pattern
                    .as_deref()
                    .is_none_or(|rule_pattern| glob_match(rule_pattern, pattern))
            {
                matched = Some(rule);
            }
        }
        matched
    }

    #[must_use]
    pub fn decide(&self, tool_call: &Value) -> PermissionAction {
        let tool = tool_call
            .get("name")
            .or_else(|| tool_call.get("tool"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let payload = tool_call.get("input").cloned().unwrap_or_else(|| json!({}));
        let pattern = pattern_for(&payload);
        self.evaluate(tool, &pattern)
            .map(|rule| rule.action.clone())
            .unwrap_or(PermissionAction::Ask)
    }

    pub fn check(&self, tool_call: &Value) -> Result<PermissionAction, String> {
        let tool = tool_call
            .get("name")
            .or_else(|| tool_call.get("tool"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        match self.decide(tool_call) {
            PermissionAction::Allow => Ok(PermissionAction::Allow),
            PermissionAction::Deny => Err(format!("Permission denied: {tool}")),
            PermissionAction::Ask => Err(format!("Permission requires user confirmation: {tool}")),
        }
    }
}

#[must_use]
pub fn permission_rule(
    tool: &str,
    action: PermissionAction,
    pattern: Option<&str>,
) -> PermissionRule {
    PermissionRule {
        tool: tool.to_string(),
        action,
        pattern: pattern.map(str::to_string),
        condition: None,
    }
}

#[must_use]
pub fn pattern_for(payload: &Value) -> String {
    if let Some(object) = payload.as_object() {
        for key in [
            "file_path",
            "filePath",
            "path",
            "pattern",
            "command",
            "subagent_type",
            "agent_type",
            "agent",
            "name",
        ] {
            if let Some(value) = object.get(key).and_then(Value::as_str)
                && !value.is_empty()
            {
                return value.to_string();
            }
        }
    }
    stable_json_dumps(payload)
}
