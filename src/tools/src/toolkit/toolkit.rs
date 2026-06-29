#[derive(Clone, Debug)]
pub struct Toolkit {
    pub registry: ToolRegistry,
    pub max_output_lines: usize,
    pub max_output_bytes: usize,
}

impl Default for Toolkit {
    fn default() -> Self {
        Self {
            registry: ToolRegistry::new(),
            max_output_lines: DEFAULT_TOOL_MAX_LINES,
            max_output_bytes: DEFAULT_TOOL_MAX_BYTES,
        }
    }
}

impl Toolkit {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_builtins() -> Self {
        let mut toolkit = Self::new();
        toolkit.load_builtin();
        toolkit
    }

    pub fn load_builtin(&mut self) {
        register_builtin_tools(&mut self.registry);
    }

    #[must_use]
    pub fn get_all_tools(&self, execution_mode: &str) -> Vec<ToolSchema> {
        self.registry.tool_schemas(execution_mode)
    }

    #[must_use]
    pub fn execute(
        &self,
        name: &str,
        input: Value,
        call_id: &str,
        ctx: &mut ToolContext,
    ) -> ToolResult {
        let Some(tool) = self.registry.get(name) else {
            return ToolResult {
                call_id: call_id.to_string(),
                output: String::new(),
                error: Some(format!("Tool not found: {name}")),
                metadata: BTreeMap::from([("tool".to_string(), json!(name))]),
            };
        };
        if !tool_available(tool, &ctx.execution_mode) {
            return ToolResult {
                call_id: call_id.to_string(),
                output: String::new(),
                error: Some(format!(
                    "Tool \"{name}\" is not available in {} mode.",
                    ctx.execution_mode
                )),
                metadata: BTreeMap::from([
                    ("tool".to_string(), json!(tool.id)),
                    ("execution_mode".to_string(), json!(ctx.execution_mode)),
                    (
                        "error_kind".to_string(),
                        json!("execution_scope_unavailable"),
                    ),
                ]),
            };
        }
        if let Some(result) = permission_gate(tool, &input, call_id, ctx) {
            return result;
        }

        ctx.call_id = call_id.to_string();
        match execute_builtin(name, input, ctx) {
            Ok(output) => self.finish_tool_result(tool, call_id, output, ctx),
            Err(error) => ToolResult {
                call_id: call_id.to_string(),
                output: String::new(),
                error: Some(error),
                metadata: BTreeMap::from([("tool".to_string(), json!(tool.id))]),
            },
        }
    }

    #[must_use]
    pub fn permission_result_for_tool(
        &self,
        name: &str,
        input: &Value,
        call_id: &str,
        ctx: &ToolContext,
    ) -> Option<ToolResult> {
        let Some(tool) = self.registry.get(name) else {
            return Some(ToolResult {
                call_id: call_id.to_string(),
                output: String::new(),
                error: Some(format!("Tool not found: {name}")),
                metadata: BTreeMap::from([("tool".to_string(), json!(name))]),
            });
        };
        if !tool_available(tool, &ctx.execution_mode) {
            return Some(ToolResult {
                call_id: call_id.to_string(),
                output: String::new(),
                error: Some(format!(
                    "Tool \"{name}\" is not available in {} mode.",
                    ctx.execution_mode
                )),
                metadata: BTreeMap::from([
                    ("tool".to_string(), json!(tool.id)),
                    ("execution_mode".to_string(), json!(ctx.execution_mode)),
                    (
                        "error_kind".to_string(),
                        json!("execution_scope_unavailable"),
                    ),
                ]),
            });
        }
        permission_gate(tool, input, call_id, ctx)
    }

    fn finish_tool_result(
        &self,
        tool: &ToolDefinition,
        call_id: &str,
        output: ToolOutput,
        ctx: &ToolContext,
    ) -> ToolResult {
        let truncated_output = truncate_output(
            &output.output,
            Some(self.max_output_lines),
            Some(self.max_output_bytes),
        );
        let output_truncated = truncated_output.truncated;
        let mut metadata = output.metadata;
        metadata
            .entry("tool".to_string())
            .or_insert_with(|| json!(tool.id));
        metadata
            .entry("title".to_string())
            .or_insert_with(|| json!(output.title));
        metadata.insert(
            "truncated".to_string(),
            json!(output.truncated || output_truncated),
        );
        metadata.insert("output_truncated".to_string(), json!(output_truncated));
        metadata.insert(
            "original_lines".to_string(),
            json!(truncated_output.original_lines),
        );
        metadata.insert(
            "original_bytes".to_string(),
            json!(truncated_output.original_bytes),
        );

        let mut output_text = truncated_output.content;
        if output_truncated {
            match write_truncated_output(&ctx.session_root, call_id, &output.output) {
                Ok(path) => {
                    metadata.insert("output_path".to_string(), json!(path_to_string(&path)));
                }
                Err(error) => {
                    output_text.push_str(&format!("\n\nFailed to save full output: {error}"));
                }
            }
        }

        ToolResult {
            call_id: call_id.to_string(),
            output: output_text,
            error: output.error,
            metadata,
        }
    }
}

fn permission_gate(
    tool: &ToolDefinition,
    input: &Value,
    call_id: &str,
    ctx: &ToolContext,
) -> Option<ToolResult> {
    let manager = ctx.permission_manager.as_ref()?;
    let tool_call = json!({"name": tool.id, "input": input});
    match manager.decide(&tool_call) {
        PermissionAction::Allow => None,
        PermissionAction::Ask if ctx.dangerously_skip_permissions => None,
        PermissionAction::Ask => Some(permission_tool_result(
            tool,
            input,
            call_id,
            PermissionFailure {
                action: PermissionAction::Ask,
                message: "Permission requires user confirmation",
                error_kind: "permission_required",
                requires_approval: true,
                dangerously_skip_permissions: ctx.dangerously_skip_permissions,
            },
        )),
        PermissionAction::Deny => Some(permission_tool_result(
            tool,
            input,
            call_id,
            PermissionFailure {
                action: PermissionAction::Deny,
                message: "Permission denied",
                error_kind: "permission_denied",
                requires_approval: false,
                dangerously_skip_permissions: ctx.dangerously_skip_permissions,
            },
        )),
    }
}

struct PermissionFailure {
    action: PermissionAction,
    message: &'static str,
    error_kind: &'static str,
    requires_approval: bool,
    dangerously_skip_permissions: bool,
}

fn permission_tool_result(
    tool: &ToolDefinition,
    input: &Value,
    call_id: &str,
    failure: PermissionFailure,
) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        output: String::new(),
        error: Some(format!("{}: {}", failure.message, tool.id)),
        metadata: BTreeMap::from([
            ("tool".to_string(), json!(tool.id)),
            (
                "permission_action".to_string(),
                json!(permission_action_name(&failure.action)),
            ),
            ("permission_pattern".to_string(), json!(pattern_for(input))),
            (
                "permission_required".to_string(),
                json!(failure.requires_approval),
            ),
            (
                "requires_approval".to_string(),
                json!(failure.requires_approval),
            ),
            (
                "dangerously_skip_permissions".to_string(),
                json!(failure.dangerously_skip_permissions),
            ),
            ("dangerous".to_string(), json!(tool.dangerous)),
            ("group".to_string(), json!(tool.group)),
            ("call_id".to_string(), json!(call_id)),
            ("input".to_string(), input.clone()),
            ("error_kind".to_string(), json!(failure.error_kind)),
        ]),
    }
}

fn permission_action_name(action: &PermissionAction) -> &'static str {
    match action {
        PermissionAction::Allow => "allow",
        PermissionAction::Deny => "deny",
        PermissionAction::Ask => "ask",
    }
}
