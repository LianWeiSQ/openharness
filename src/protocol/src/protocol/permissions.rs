#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PermissionRule {
    pub tool: String,
    pub action: PermissionAction,
    pub pattern: Option<String>,
    pub condition: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub enum PermissionRuleset {
    #[serde(rename = "FULL")]
    Full,
    #[serde(rename = "READONLY")]
    Readonly,
    #[serde(rename = "PLAN_ONLY")]
    PlanOnly,
    #[serde(rename = "NONE")]
    None,
}

impl PermissionRuleset {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "FULL",
            Self::Readonly => "READONLY",
            Self::PlanOnly => "PLAN_ONLY",
            Self::None => "NONE",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PermissionRulesetDef {
    pub name: PermissionRuleset,
    pub rules: Vec<PermissionRule>,
}

#[must_use]
pub fn ruleset(name: PermissionRuleset) -> PermissionRulesetDef {
    let rules = match name {
        PermissionRuleset::Full => vec![permission_rule("*", PermissionAction::Allow)],
        PermissionRuleset::Readonly => {
            let mut rules = vec![permission_rule("*", PermissionAction::Deny)];
            rules.extend(
                readonly_tools().map(|tool| permission_rule(tool, PermissionAction::Allow)),
            );
            rules
        }
        PermissionRuleset::PlanOnly => {
            let mut rules = vec![permission_rule("*", PermissionAction::Ask)];
            rules.extend(
                plan_only_tools().map(|tool| permission_rule(tool, PermissionAction::Allow)),
            );
            rules
        }
        PermissionRuleset::None => vec![permission_rule("*", PermissionAction::Deny)],
    };
    PermissionRulesetDef { name, rules }
}
