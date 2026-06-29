#[derive(Clone, Debug, Default)]
pub struct ContextPackInput {
    pub messages: Vec<ChatMessage>,
    pub metadata: BTreeMap<String, Value>,
    pub todos: Vec<Value>,
    pub runtime_context: Option<String>,
    pub sandbox_metadata: Option<Value>,
    pub extra_items: Vec<ContextItem>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InstructionLoadOptions {
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
    pub include_user: bool,
    pub user_config_dir: Option<PathBuf>,
    pub workspace_files: Vec<String>,
    pub user_files: Vec<String>,
}

impl Default for InstructionLoadOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            include_user: true,
            user_config_dir: None,
            workspace_files: DEFAULT_WORKSPACE_FILES
                .iter()
                .map(|item| (*item).to_string())
                .collect(),
            user_files: DEFAULT_USER_FILES
                .iter()
                .map(|item| (*item).to_string())
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InstructionItem {
    pub path: String,
    pub display_path: String,
    pub source: String,
    pub scope: String,
    pub content: String,
    pub bytes_read: usize,
    pub truncated: bool,
}

impl InstructionItem {
    #[must_use]
    pub fn to_context_item(&self) -> ContextItem {
        let digest = sha1_hex_12(&self.path);
        let mut metadata = BTreeMap::new();
        metadata.insert("path".to_string(), json!(self.path));
        metadata.insert("display_path".to_string(), json!(self.display_path));
        metadata.insert("scope".to_string(), json!(self.scope));
        metadata.insert("bytes_read".to_string(), json!(self.bytes_read));
        metadata.insert("truncated".to_string(), json!(self.truncated));
        let mut item = ContextItem::new(
            format!("instruction:{}:{digest}", self.scope),
            "instruction",
            self.source.clone(),
            format!("[Instruction: {}]\n{}", self.display_path, self.content)
                .trim()
                .to_string(),
            100,
        );
        item.pinned = true;
        item.stable_prefix = true;
        item.metadata = metadata;
        item
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InstructionContext {
    pub items: Vec<InstructionItem>,
    pub total_bytes: usize,
    pub truncated: bool,
    pub issues: Vec<String>,
}

impl InstructionContext {
    #[must_use]
    pub fn to_context_items(&self) -> Vec<ContextItem> {
        self.items
            .iter()
            .map(InstructionItem::to_context_item)
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct InstructionContextLoader {
    workspace_root: PathBuf,
    options: InstructionLoadOptions,
}

impl InstructionContextLoader {
    #[must_use]
    pub fn new(
        workspace_root: impl Into<PathBuf>,
        options: Option<InstructionLoadOptions>,
    ) -> Self {
        let root = canonicalize_existing(&workspace_root.into());
        Self {
            workspace_root: root,
            options: options.unwrap_or_default(),
        }
    }

    #[must_use]
    pub fn load(&self) -> InstructionContext {
        let mut issues = Vec::new();
        let mut items = Vec::new();
        let mut total_bytes = 0usize;
        let mut truncated = false;
        let mut seen = BTreeSet::new();
        for candidate in self.candidates() {
            let path = canonicalize_existing(&candidate.path);
            if seen.contains(&path) || !path.is_file() {
                continue;
            }
            seen.insert(path.clone());
            if !self.is_allowed_path(&path) {
                issues.push(format!("skipped_out_of_scope:{}", candidate.display_path));
                continue;
            }
            if total_bytes >= self.options.max_total_bytes {
                truncated = true;
                issues.push("total_limit_reached".to_string());
                break;
            }
            match self.load_candidate(&candidate, self.options.max_total_bytes - total_bytes) {
                Some((item, issue)) => {
                    if let Some(issue) = issue {
                        issues.push(issue);
                    }
                    total_bytes += item.bytes_read;
                    truncated |= item.truncated;
                    items.push(item);
                }
                None => issues.push(format!("skipped_unreadable:{}", candidate.display_path)),
            }
        }
        InstructionContext {
            items,
            total_bytes,
            truncated,
            issues,
        }
    }

    fn candidates(&self) -> Vec<InstructionCandidate> {
        let mut candidates = Vec::new();
        for base in self.workspace_ancestors() {
            for filename in &self.options.workspace_files {
                let path = base.join(filename);
                let display = self.display_workspace_path(&path);
                candidates.push(InstructionCandidate {
                    path,
                    display_path: display.clone(),
                    source: format!("instructions.workspace:{display}"),
                    scope: "workspace".to_string(),
                });
            }
            let path = base.join(".openagent").join("instructions.md");
            let display = self.display_workspace_path(&path);
            candidates.push(InstructionCandidate {
                path,
                display_path: display.clone(),
                source: format!("instructions.workspace:{display}"),
                scope: "workspace".to_string(),
            });
            let rules_dir = base.join(".openagent").join("rules");
            let mut rules = read_dir_paths(&rules_dir)
                .into_iter()
                .filter(|path| path.extension().and_then(OsStr::to_str) == Some("md"))
                .collect::<Vec<_>>();
            rules.sort();
            for rule in rules {
                let display = self.display_workspace_path(&rule);
                candidates.push(InstructionCandidate {
                    path: rule,
                    display_path: display.clone(),
                    source: format!("instructions.workspace:{display}"),
                    scope: "workspace".to_string(),
                });
            }
        }
        if self.options.include_user {
            let user_dir = self.user_config_dir();
            for filename in &self.options.user_files {
                candidates.push(InstructionCandidate {
                    path: user_dir.join(filename),
                    display_path: format!("~/.openagent/{filename}"),
                    source: format!("instructions.user:{filename}"),
                    scope: "user".to_string(),
                });
            }
            let mut rules = read_dir_paths(&user_dir.join("rules"))
                .into_iter()
                .filter(|path| path.extension().and_then(OsStr::to_str) == Some("md"))
                .collect::<Vec<_>>();
            rules.sort();
            for rule in rules {
                let name = rule
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or_default()
                    .to_string();
                candidates.push(InstructionCandidate {
                    path: rule,
                    display_path: format!("~/.openagent/rules/{name}"),
                    source: format!("instructions.user:rules/{name}"),
                    scope: "user".to_string(),
                });
            }
        }
        candidates
    }

    fn load_candidate(
        &self,
        candidate: &InstructionCandidate,
        remaining_bytes: usize,
    ) -> Option<(InstructionItem, Option<String>)> {
        let raw = fs::read(&candidate.path).ok()?;
        if raw.iter().take(1024).any(|byte| *byte == 0) {
            return None;
        }
        if std::str::from_utf8(&raw).is_err() {
            return None;
        }
        let mut allowed = raw
            .len()
            .min(self.options.max_file_bytes)
            .min(remaining_bytes);
        while allowed > 0 && std::str::from_utf8(&raw[..allowed]).is_err() {
            allowed -= 1;
        }
        if allowed == 0 {
            return None;
        }
        let truncated = allowed < raw.len();
        let content = std::str::from_utf8(&raw[..allowed])
            .ok()?
            .trim()
            .to_string();
        let path = canonicalize_existing(&candidate.path);
        let issue = truncated.then(|| format!("truncated:{}", candidate.display_path));
        Some((
            InstructionItem {
                path: path_to_string(&path),
                display_path: candidate.display_path.clone(),
                source: candidate.source.clone(),
                scope: candidate.scope.clone(),
                content,
                bytes_read: allowed,
                truncated,
            },
            issue,
        ))
    }

    fn workspace_ancestors(&self) -> Vec<PathBuf> {
        let mut result = vec![self.workspace_root.clone()];
        result.extend(
            self.workspace_root
                .ancestors()
                .skip(1)
                .map(Path::to_path_buf),
        );
        result
    }

    fn user_config_dir(&self) -> PathBuf {
        self.options
            .user_config_dir
            .as_ref()
            .map(|path| canonicalize_existing(path))
            .unwrap_or_else(|| default_home_dir().join(".openagent"))
    }

    fn is_allowed_path(&self, path: &Path) -> bool {
        if path.starts_with(&self.workspace_root) {
            return true;
        }
        for ancestor in self.workspace_root.ancestors().skip(1) {
            if path.parent() == Some(ancestor) || path.starts_with(ancestor.join(".openagent")) {
                return true;
            }
        }
        self.options.include_user && path.starts_with(self.user_config_dir())
    }

    fn display_workspace_path(&self, path: &Path) -> String {
        let resolved = canonicalize_existing(path);
        resolved
            .strip_prefix(&self.workspace_root)
            .map(path_to_string)
            .unwrap_or_else(|_| {
                resolved
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or_default()
                    .to_string()
            })
    }
}

#[derive(Clone, Debug)]
struct InstructionCandidate {
    path: PathBuf,
    display_path: String,
    source: String,
    scope: String,
}
