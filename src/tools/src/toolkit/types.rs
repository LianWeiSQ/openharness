#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn protocol_crate_name() -> &'static str {
    openagent_protocol::crate_name()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolDefinition {
    pub id: String,
    pub description: String,
    pub parameter_schema: Value,
    pub dangerous: bool,
    pub group: String,
    pub execution_scope: ToolExecutionScope,
    pub execution_schema: ToolExecutionSchema,
}

impl ToolDefinition {
    #[must_use]
    pub fn tool_schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.id.clone(),
            description: self.description.clone(),
            schema: Some(self.parameter_schema.clone()),
            group: self.group.clone(),
            dangerous: self.dangerous,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, ToolDefinition>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn scoped(&mut self, namespace: impl Into<String>) -> ScopedToolRegistry<'_> {
        ScopedToolRegistry {
            registry: self,
            namespace: namespace.into(),
        }
    }

    pub fn register(&mut self, tool: ToolDefinition) {
        self.tools.insert(tool.id.clone(), tool);
    }

    #[must_use]
    pub fn get(&self, tool_id: &str) -> Option<&ToolDefinition> {
        self.tools.get(tool_id)
    }

    #[must_use]
    pub fn all(&self) -> Vec<&ToolDefinition> {
        self.tools.values().collect()
    }

    pub fn clear(&mut self) {
        self.tools.clear();
    }

    #[must_use]
    pub fn tool_schemas(&self, execution_mode: &str) -> Vec<ToolSchema> {
        self.tools
            .values()
            .filter(|tool| tool_available(tool, execution_mode))
            .map(ToolDefinition::tool_schema)
            .collect()
    }
}

pub struct ScopedToolRegistry<'a> {
    registry: &'a mut ToolRegistry,
    namespace: String,
}

impl ScopedToolRegistry<'_> {
    pub fn register(&mut self, mut tool: ToolDefinition) {
        tool.id = qualify_tool_id(&self.namespace, &tool.id);
        self.registry.register(tool);
    }
}

#[must_use]
pub fn qualify_tool_id(namespace: &str, tool_id: &str) -> String {
    if tool_id == "default" {
        namespace.to_string()
    } else {
        format!("{namespace}_{tool_id}")
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TaskSubagentDescriptor {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[must_use]
pub fn task_tool_description(subagents: &[TaskSubagentDescriptor]) -> String {
    let mut description = [
        "Launch a subagent in an independent context window for complex, multi-step work.",
        "Use this when the task is best handled by a specialized agent whose intermediate tool calls should not enter the parent context.",
        "Pass a compact description, the full prompt for the subagent, and subagent_type.",
        "Do not use this for simple one-shot file reads or searches that local tools can handle directly.",
        "The parent agent must summarize the returned result for the user when needed.",
    ]
    .join("\n");
    if subagents.is_empty() {
        description.push_str("\n\nAvailable subagents: none.");
    } else {
        description.push_str("\n\nAvailable subagents:");
        for agent in subagents {
            let summary = if agent.description.trim().is_empty() {
                "No description provided."
            } else {
                agent.description.trim()
            };
            description.push_str(&format!("\n- {}: {}", agent.id, summary));
        }
    }
    description
}

#[must_use]
pub fn task_tool_definition(subagents: &[TaskSubagentDescriptor]) -> ToolDefinition {
    ToolDefinition {
        id: TASK_TOOL_ID.to_string(),
        description: task_tool_description(subagents),
        parameter_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short title for the delegated task."
                },
                "prompt": {
                    "type": "string",
                    "description": "The complete instructions for the subagent."
                },
                "subagent_type": {
                    "type": "string",
                    "description": "The subagent id to run."
                },
                "task_id": {
                    "type": "string",
                    "description": "Optional existing subagent session id to resume."
                },
                "command": {
                    "type": "string",
                    "description": "Optional command label for the subagent task."
                },
                "background": {
                    "type": "boolean",
                    "description": "Reserved for background execution; current runtime executes foreground tasks."
                }
            },
            "required": ["description", "prompt", "subagent_type"]
        }),
        dangerous: true,
        group: "agent".to_string(),
        execution_scope: ToolExecutionScope::Agnostic,
        execution_schema: exclusive_schema(
            "agent",
            false,
            true,
            false,
            false,
            false,
            Some("task:{subagent_type}"),
        ),
    }
}

pub fn register_task_tool(registry: &mut ToolRegistry, subagents: &[TaskSubagentDescriptor]) {
    registry.register(task_tool_definition(subagents));
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ToolOutput {
    pub title: String,
    pub output: String,
    pub metadata: BTreeMap<String, Value>,
    pub truncated: bool,
    pub error: Option<String>,
}

impl ToolOutput {
    #[must_use]
    pub fn new(title: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            output: output.into(),
            metadata: BTreeMap::new(),
            truncated: false,
            error: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    pub priority: String,
    pub id: String,
}

impl TodoItem {
    #[must_use]
    pub fn new(
        content: impl Into<String>,
        status: impl Into<String>,
        priority: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            status: status.into(),
            priority: priority.into(),
            id: id.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolContext {
    pub session_id: String,
    pub session_root: PathBuf,
    pub call_id: String,
    pub execution_mode: String,
    pub workspace_root: Option<String>,
    pub execution_metadata: BTreeMap<String, Value>,
    pub agent_options: BTreeMap<String, Value>,
    pub require_read_before_write: bool,
    pub permission_manager: Option<PermissionManager>,
    pub dangerously_skip_permissions: bool,
    pub read_files: BTreeSet<PathBuf>,
    pub memory: BTreeMap<String, Value>,
    pub todos: Vec<TodoItem>,
    pub question_answers: Option<Vec<Vec<String>>>,
}

impl ToolContext {
    #[must_use]
    pub fn new(session_root: impl Into<PathBuf>) -> Self {
        let root = root_path(&session_root.into());
        Self {
            session_id: String::new(),
            session_root: root.clone(),
            call_id: String::new(),
            execution_mode: "local".to_string(),
            workspace_root: Some(root.to_string_lossy().to_string()),
            execution_metadata: BTreeMap::new(),
            agent_options: BTreeMap::new(),
            require_read_before_write: true,
            permission_manager: None,
            dangerously_skip_permissions: false,
            read_files: BTreeSet::new(),
            memory: BTreeMap::new(),
            todos: Vec::new(),
            question_answers: None,
        }
    }

    #[must_use]
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    #[must_use]
    pub fn with_call_id(mut self, call_id: impl Into<String>) -> Self {
        self.call_id = call_id.into();
        self
    }

    #[must_use]
    pub fn with_read_before_write(mut self, enabled: bool) -> Self {
        self.require_read_before_write = enabled;
        self
    }

    #[must_use]
    pub fn with_permission_manager(mut self, manager: PermissionManager) -> Self {
        self.permission_manager = Some(manager);
        self
    }

    #[must_use]
    pub fn with_permission_ruleset(mut self, ruleset: PermissionRuleset) -> Self {
        let mut manager = PermissionManager::new();
        manager.set_ruleset(ruleset);
        self.permission_manager = Some(manager);
        self
    }

    #[must_use]
    pub fn with_dangerously_skip_permissions(mut self, enabled: bool) -> Self {
        self.dangerously_skip_permissions = enabled;
        self
    }

    pub fn set_question_answers(&mut self, answers: Vec<Vec<String>>) {
        self.question_answers = Some(answers);
    }

    pub fn remember_read(&mut self, target: &Path) {
        self.read_files.insert(normalize_path(target));
    }

    #[must_use]
    pub fn has_read_file(&self, target: &Path) -> bool {
        self.read_files.contains(&normalize_path(target))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CommandResult {
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
    pub cwd: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkspaceEntry {
    pub path: String,
    pub is_dir: bool,
    pub mtime: f64,
}

#[derive(Clone, Debug)]
pub struct LocalWorkspaceRuntime {
    pub root: PathBuf,
    pub workspace_root: String,
}

impl LocalWorkspaceRuntime {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root_path(&root.into());
        Self {
            workspace_root: root.to_string_lossy().to_string(),
            root,
        }
    }

    #[must_use]
    pub fn execution_metadata(&self) -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("execution_mode".to_string(), json!("local")),
            ("workspace_root".to_string(), json!(self.workspace_root)),
        ])
    }

    #[must_use]
    pub fn display_path(&self, path: impl AsRef<Path>) -> String {
        let target = normalize_path(path.as_ref());
        target
            .strip_prefix(&self.root)
            .map(path_to_string)
            .unwrap_or_else(|_| path_to_string(&target))
    }

    pub fn resolve_path(
        &self,
        path: Option<&str>,
        default_to_root: bool,
    ) -> ToolResultValue<PathBuf> {
        match path {
            Some(raw) => resolve_optional_path(&self.root, Some(raw)),
            None if default_to_root => resolve_optional_path(&self.root, None),
            None => Err("path is required".to_string()),
        }
    }

    pub fn resolve_file_path(&self, path: &str) -> ToolResultValue<PathBuf> {
        resolve_path_in_root(&self.root, path)
    }

    pub fn run_command(
        &self,
        command: &str,
        cwd: Option<&str>,
        _timeout_ms: u64,
    ) -> ToolResultValue<CommandResult> {
        let resolved_cwd = self.resolve_path(cwd, true)?;
        let output = shell_command(command)
            .current_dir(&resolved_cwd)
            .output()
            .map_err(|error| format!("failed to run command: {error}"))?;
        Ok(CommandResult {
            returncode: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            cwd: path_to_string(&resolved_cwd),
        })
    }

    pub fn read_text(&self, path: &str) -> ToolResultValue<String> {
        fs::read_to_string(self.resolve_file_path(path)?).map_err(io_error)
    }

    pub fn write_text(&self, path: &str, content: &str) -> ToolResultValue<()> {
        let target = self.resolve_file_path(path)?;
        write_text(&target, content)
    }

    pub fn edit_text(
        &self,
        path: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> ToolResultValue<String> {
        let target = self.resolve_file_path(path)?;
        let text = fs::read_to_string(&target).map_err(io_error)?;
        let new_text = replace_text(&text, old_string, new_string, replace_all)?;
        write_text(&target, &new_text)?;
        Ok(new_text)
    }

    pub fn glob(&self, base_path: Option<&str>, pattern: &str) -> ToolResultValue<Vec<PathBuf>> {
        let base = self.resolve_path(base_path, true)?;
        let mut matches = glob_paths(&self.root, &base, pattern)?;
        matches.sort_by(|left, right| {
            path_mtime(right)
                .total_cmp(&path_mtime(left))
                .then_with(|| path_to_string(left).cmp(&path_to_string(right)))
        });
        matches.truncate(GLOB_LIMIT);
        Ok(matches)
    }

    pub fn grep(
        &self,
        base_path: Option<&str>,
        pattern: &str,
        include_glob: Option<&str>,
    ) -> ToolResultValue<Vec<GrepMatch>> {
        let base = self.resolve_path(base_path, true)?;
        grep_paths(&self.root, &base, pattern, include_glob)
    }

    pub fn ls(
        &self,
        base_path: Option<&str>,
        ignore: &[String],
    ) -> ToolResultValue<Vec<WorkspaceEntry>> {
        let base = self.resolve_path(base_path, true)?;
        let mut entries = Vec::new();
        for path in walk_paths(&base)? {
            if path == base {
                continue;
            }
            let relative = path
                .strip_prefix(&base)
                .map(path_to_string)
                .unwrap_or_else(|_| path_to_string(&path));
            let name = path
                .file_name()
                .and_then(OsStr::to_str)
                .map(str::to_string)
                .unwrap_or_default();
            if should_ignore(&relative, &name, ignore) {
                continue;
            }
            entries.push(WorkspaceEntry {
                path: path_to_string(&path),
                is_dir: path.is_dir(),
                mtime: path_mtime(&path),
            });
            if entries.len() >= LS_LIMIT {
                break;
            }
        }
        Ok(entries)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TruncatedResult {
    pub content: String,
    pub truncated: bool,
    pub original_lines: usize,
    pub original_bytes: usize,
    pub output_path: Option<String>,
}

#[must_use]
pub fn truncate_output(
    content: &str,
    max_lines: Option<usize>,
    max_bytes: Option<usize>,
) -> TruncatedResult {
    let resolved_max_lines = max_lines.unwrap_or(DEFAULT_TOOL_MAX_LINES);
    let resolved_max_bytes = max_bytes.unwrap_or(DEFAULT_TOOL_MAX_BYTES);
    let original_lines = if content.is_empty() {
        0
    } else {
        content.matches('\n').count() + 1
    };
    let original_bytes = content.len();

    if original_lines <= resolved_max_lines && original_bytes <= resolved_max_bytes {
        return TruncatedResult {
            content: content.to_string(),
            truncated: false,
            original_lines,
            original_bytes,
            output_path: None,
        };
    }

    let mut rendered = content.to_string();
    let lines = content.split('\n').collect::<Vec<_>>();
    if lines.len() > resolved_max_lines {
        rendered = lines[..resolved_max_lines].join("\n");
        rendered.push_str(&format!(
            "\n\n... output truncated (original {original_lines} lines, showing first {resolved_max_lines} lines)"
        ));
    }

    if rendered.len() > resolved_max_bytes {
        let mut end = resolved_max_bytes.min(rendered.len());
        while end > 0 && !rendered.is_char_boundary(end) {
            end -= 1;
        }
        rendered = rendered[..end].to_string();
        rendered.push_str(&format!(
            "\n\n... output truncated (original {original_bytes} bytes)"
        ));
    }

    TruncatedResult {
        content: rendered,
        truncated: true,
        original_lines,
        original_bytes,
        output_path: None,
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReadFormat {
    pub output: String,
    pub preview: String,
    pub truncated: bool,
}

#[must_use]
pub fn format_read_output_from_text(text: &str, offset: usize, limit: usize) -> ReadFormat {
    let lines = if text.is_empty() {
        Vec::new()
    } else {
        text.lines().collect::<Vec<_>>()
    };
    let total_lines = lines.len();
    let start = offset;
    let max_lines = limit;
    let mut raw = Vec::new();
    let mut preview_lines = Vec::new();
    let mut bytes_used = 0usize;
    let mut truncated_by_bytes = false;

    for line in lines.iter().skip(start).take(max_lines) {
        let mut rendered = (*line).to_string();
        if rendered.len() > MAX_LINE_LENGTH {
            rendered.truncate(MAX_LINE_LENGTH);
            rendered.push_str("...");
        }
        let encoded_size = rendered.len() + usize::from(!raw.is_empty());
        if bytes_used + encoded_size > MAX_READ_BYTES {
            truncated_by_bytes = true;
            break;
        }
        bytes_used += encoded_size;
        if preview_lines.len() < 20 {
            preview_lines.push(rendered.clone());
        }
        raw.push(rendered);
    }

    let numbered = raw
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{:05}| {line}", index + start + 1))
        .collect::<Vec<_>>();
    let last_read_line = start + raw.len();
    let has_more_lines = total_lines > last_read_line;
    let truncated = truncated_by_bytes || has_more_lines;

    let mut output_lines = vec!["<file>".to_string()];
    output_lines.extend(numbered);
    output_lines.push(String::new());
    if truncated_by_bytes {
        output_lines.push(format!(
            "(Output truncated at {MAX_READ_BYTES} bytes. Use 'offset' parameter to read beyond line {last_read_line})"
        ));
    } else if has_more_lines {
        output_lines.push(format!(
            "(File has more lines. Use 'offset' parameter to read beyond line {last_read_line})"
        ));
    } else {
        output_lines.push(format!("(End of file - total {total_lines} lines)"));
    }
    output_lines.push("</file>".to_string());

    ReadFormat {
        output: output_lines.join("\n"),
        preview: preview_lines.join("\n"),
        truncated,
    }
}
