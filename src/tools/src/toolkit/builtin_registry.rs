pub fn register_builtin_tools(registry: &mut ToolRegistry) {
    let workspace_read = readonly_schema("workspace-read", false, false, None);
    let file_read = readonly_schema("workspace-read", false, true, None);
    let file_write = exclusive_schema(
        "workspace-write",
        true,
        true,
        false,
        false,
        false,
        Some("file:{file_path}"),
    );

    registry.register(ToolDefinition {
        id: "read".to_string(),
        description: "Read a UTF-8 workspace file.".to_string(),
        parameter_schema: schema(&["file_path"], &["file_path", "offset", "limit"]),
        dangerous: false,
        group: "file".to_string(),
        execution_scope: ToolExecutionScope::Workspace,
        execution_schema: file_read,
    });
    registry.register(ToolDefinition {
        id: "write".to_string(),
        description: "Write a UTF-8 workspace file.".to_string(),
        parameter_schema: schema(&["file_path", "content"], &["file_path", "content"]),
        dangerous: true,
        group: "file".to_string(),
        execution_scope: ToolExecutionScope::Workspace,
        execution_schema: file_write.clone(),
    });
    registry.register(ToolDefinition {
        id: "edit".to_string(),
        description: "Edit a UTF-8 workspace file.".to_string(),
        parameter_schema: schema(
            &["file_path", "old_string", "new_string"],
            &["file_path", "old_string", "new_string", "replace_all"],
        ),
        dangerous: true,
        group: "file".to_string(),
        execution_scope: ToolExecutionScope::Workspace,
        execution_schema: file_write,
    });
    registry.register(ToolDefinition {
        id: "glob".to_string(),
        description: "Find workspace paths by glob pattern.".to_string(),
        parameter_schema: schema(&["pattern"], &["pattern", "path"]),
        dangerous: false,
        group: "file".to_string(),
        execution_scope: ToolExecutionScope::Workspace,
        execution_schema: workspace_read.clone(),
    });
    registry.register(ToolDefinition {
        id: "grep".to_string(),
        description: "Search workspace files with a regular expression.".to_string(),
        parameter_schema: schema(&["pattern"], &["pattern", "path", "glob", "include"]),
        dangerous: false,
        group: "file".to_string(),
        execution_scope: ToolExecutionScope::Workspace,
        execution_schema: workspace_read.clone(),
    });
    registry.register(ToolDefinition {
        id: "ls".to_string(),
        description: "List a workspace directory tree.".to_string(),
        parameter_schema: schema(&[], &["path", "ignore"]),
        dangerous: false,
        group: "file".to_string(),
        execution_scope: ToolExecutionScope::Workspace,
        execution_schema: workspace_read.clone(),
    });
    registry.register(ToolDefinition {
        id: "bash".to_string(),
        description: "Run a shell command in the workspace.".to_string(),
        parameter_schema: schema(
            &["command"],
            &["command", "timeout", "workdir", "description"],
        ),
        dangerous: true,
        group: "shell".to_string(),
        execution_scope: ToolExecutionScope::Workspace,
        execution_schema: exclusive_schema("shell", true, true, true, true, false, None),
    });
    registry.register(ToolDefinition {
        id: "skill".to_string(),
        description: "Load or list Markdown skills discovered from the workspace.".to_string(),
        parameter_schema: schema(
            &[],
            &[
                "name",
                "query",
                "limit",
                "include_content",
                "include_diagnostics",
            ],
        ),
        dangerous: false,
        group: "skill".to_string(),
        execution_scope: ToolExecutionScope::Agnostic,
        execution_schema: readonly_schema("skill", false, false, None),
    });
    registry.register(ToolDefinition {
        id: "code_search".to_string(),
        description: "Search files for a literal substring.".to_string(),
        parameter_schema: schema(&["query"], &["query", "glob", "path"]),
        dangerous: false,
        group: "search".to_string(),
        execution_scope: ToolExecutionScope::HostOnly,
        execution_schema: workspace_read,
    });
    registry.register(ToolDefinition {
        id: "memory_read".to_string(),
        description: "Read a JSON memory value.".to_string(),
        parameter_schema: schema(&["key"], &["key"]),
        dangerous: false,
        group: "memory".to_string(),
        execution_scope: ToolExecutionScope::Agnostic,
        execution_schema: readonly_schema("memory", false, false, None),
    });
    registry.register(ToolDefinition {
        id: "memory_write".to_string(),
        description: "Write a JSON memory value.".to_string(),
        parameter_schema: schema(&["key", "value"], &["key", "value"]),
        dangerous: false,
        group: "memory".to_string(),
        execution_scope: ToolExecutionScope::Agnostic,
        execution_schema: exclusive_schema(
            "memory",
            false,
            true,
            false,
            false,
            false,
            Some("memory:{key}"),
        ),
    });
    registry.register(ToolDefinition {
        id: "todowrite".to_string(),
        description: "Replace the session todo list.".to_string(),
        parameter_schema: schema(&["todos"], &["todos"]),
        dangerous: false,
        group: "todo".to_string(),
        execution_scope: ToolExecutionScope::Agnostic,
        execution_schema: exclusive_schema(
            "todo",
            false,
            true,
            false,
            false,
            false,
            Some("session:todos"),
        ),
    });
    registry.register(ToolDefinition {
        id: "todoread".to_string(),
        description: "Read the session todo list.".to_string(),
        parameter_schema: schema(&[], &[]),
        dangerous: false,
        group: "todo".to_string(),
        execution_scope: ToolExecutionScope::Agnostic,
        execution_schema: readonly_schema("todo", false, false, None),
    });
    registry.register(ToolDefinition {
        id: "question".to_string(),
        description: "Ask the user structured questions.".to_string(),
        parameter_schema: schema(&["questions"], &["questions"]),
        dangerous: false,
        group: "interactive".to_string(),
        execution_scope: ToolExecutionScope::Agnostic,
        execution_schema: exclusive_schema("interactive", false, true, false, false, true, None),
    });
}

#[must_use]
pub fn readonly_schema(
    batch_group: impl Into<String>,
    external_io: bool,
    mutates_session: bool,
    max_parallelism: Option<u64>,
) -> ToolExecutionSchema {
    ToolExecutionSchema {
        read_only: true,
        mutates_workspace: false,
        mutates_session,
        mutates_external: false,
        external_io,
        requires_user_interaction: false,
        concurrency: ToolConcurrency::Safe,
        batch_group: batch_group.into(),
        conflict_key_template: None,
        max_parallelism,
    }
}

#[must_use]
pub fn exclusive_schema(
    batch_group: impl Into<String>,
    mutates_workspace: bool,
    mutates_session: bool,
    mutates_external: bool,
    external_io: bool,
    requires_user_interaction: bool,
    conflict_key_template: Option<&str>,
) -> ToolExecutionSchema {
    ToolExecutionSchema {
        read_only: false,
        mutates_workspace,
        mutates_session,
        mutates_external,
        external_io,
        requires_user_interaction,
        concurrency: ToolConcurrency::Exclusive,
        batch_group: batch_group.into(),
        conflict_key_template: conflict_key_template.map(str::to_string),
        max_parallelism: None,
    }
}
