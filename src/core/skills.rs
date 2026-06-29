#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub location: String,
    pub directory: String,
    pub metadata: BTreeMap<String, Value>,
    pub score: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SkillDocument {
    pub name: String,
    pub description: String,
    pub location: String,
    pub directory: String,
    pub metadata: BTreeMap<String, Value>,
    pub score: Option<i64>,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SkillIssue {
    pub kind: String,
    pub path: String,
    pub message: String,
    pub duplicate_of: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SkillDiscoveryReport {
    pub skills: Vec<SkillInfo>,
    pub scanned_files: u64,
    pub loaded_count: u64,
    pub invalid_count: u64,
    pub duplicate_count: u64,
    pub issues: Vec<SkillIssue>,
}

#[derive(Clone, Debug)]
pub struct SkillRegistry {
    session_root: PathBuf,
    roots: Vec<String>,
    home_dir: PathBuf,
}

impl SkillRegistry {
    #[must_use]
    pub fn new(
        session_root: Option<impl Into<PathBuf>>,
        roots: Option<Vec<String>>,
        home_dir: Option<impl Into<PathBuf>>,
    ) -> Self {
        Self {
            session_root: canonicalize_existing(
                &session_root.map(Into::into).unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                }),
            ),
            roots: roots.unwrap_or_default(),
            home_dir: canonicalize_existing(
                &home_dir.map(Into::into).unwrap_or_else(default_home_dir),
            ),
        }
    }

    #[must_use]
    pub fn all(&self) -> Vec<SkillInfo> {
        self.discover()
            .documents
            .values()
            .map(|document| to_skill_info(document, None))
            .collect()
    }

    #[must_use]
    pub fn search(&self, query: &str, limit: Option<usize>) -> Vec<SkillInfo> {
        let terms = query_terms(query);
        if terms.is_empty() {
            let all = self.all();
            return limit.map_or(all.clone(), |limit| all.into_iter().take(limit).collect());
        }
        let mut scored = self
            .discover()
            .documents
            .values()
            .filter_map(|document| {
                let score = score_document(document, &terms);
                (score > 0).then(|| to_skill_info(document, Some(score)))
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .score
                .unwrap_or(0)
                .cmp(&left.score.unwrap_or(0))
                .then_with(|| left.name.cmp(&right.name))
        });
        limit.map_or(scored.clone(), |limit| {
            scored.into_iter().take(limit).collect()
        })
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<SkillDocument> {
        self.discover().documents.remove(name.trim())
    }

    #[must_use]
    pub fn report(&self, query: Option<&str>, limit: Option<usize>) -> SkillDiscoveryReport {
        let discovery = self.discover();
        let mut skills = if let Some(query) = query.filter(|query| !query.trim().is_empty()) {
            let terms = query_terms(query);
            discovery
                .documents
                .values()
                .filter_map(|document| {
                    let score = score_document(document, &terms);
                    (score > 0).then(|| to_skill_info(document, Some(score)))
                })
                .collect::<Vec<_>>()
        } else {
            discovery
                .documents
                .values()
                .map(|document| to_skill_info(document, None))
                .collect::<Vec<_>>()
        };
        if query.is_some() {
            skills.sort_by(|left, right| {
                right
                    .score
                    .unwrap_or(0)
                    .cmp(&left.score.unwrap_or(0))
                    .then_with(|| left.name.cmp(&right.name))
            });
        }
        if let Some(limit) = limit {
            skills.truncate(limit);
        }
        let invalid_count = discovery
            .issues
            .iter()
            .filter(|issue| issue.kind == "invalid")
            .count() as u64;
        let duplicate_count = discovery
            .issues
            .iter()
            .filter(|issue| issue.kind == "duplicate")
            .count() as u64;
        SkillDiscoveryReport {
            skills,
            scanned_files: discovery.scanned_files,
            loaded_count: discovery.documents.len() as u64,
            invalid_count,
            duplicate_count,
            issues: discovery.issues,
        }
    }

    fn discover(&self) -> DiscoveryResult {
        let mut documents: BTreeMap<String, SkillDocument> = BTreeMap::new();
        let mut issues = Vec::new();
        let mut scanned_files = 0u64;
        for path in self.iter_skill_files() {
            scanned_files += 1;
            let document = match load_skill_document(&path) {
                Ok(document) => document,
                Err(error) => {
                    issues.push(SkillIssue {
                        kind: "invalid".to_string(),
                        path: path_to_string(&path),
                        message: error,
                        duplicate_of: None,
                    });
                    continue;
                }
            };
            if let Some(existing) = documents.get(&document.name) {
                issues.push(SkillIssue {
                    kind: "duplicate".to_string(),
                    path: document.location.clone(),
                    message: format!("Duplicate skill name: {}", document.name),
                    duplicate_of: Some(existing.location.clone()),
                });
                continue;
            }
            documents.insert(document.name.clone(), document);
        }
        DiscoveryResult {
            documents,
            issues,
            scanned_files,
        }
    }

    fn iter_skill_files(&self) -> Vec<PathBuf> {
        if !self.roots.is_empty() {
            return self.iter_explicit_skill_files();
        }
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for base in self.workspace_ancestors() {
            result.extend(iter_pattern_matches(&base, &mut seen));
        }
        result.extend(iter_pattern_matches(&self.home_dir, &mut seen));
        result
    }

    fn iter_explicit_skill_files(&self) -> Vec<PathBuf> {
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for raw_root in &self.roots {
            let raw = PathBuf::from(raw_root);
            let root = if raw.is_absolute() {
                canonicalize_existing(&raw)
            } else {
                canonicalize_existing(&self.session_root.join(raw))
            };
            if root.is_file() && root.file_name().and_then(OsStr::to_str) == Some("SKILL.md") {
                if seen.insert(root.clone()) {
                    result.push(root);
                }
                continue;
            }
            if root.is_dir() {
                for path in recursive_skill_files(&root) {
                    if seen.insert(path.clone()) {
                        result.push(path);
                    }
                }
            }
        }
        result
    }

    fn workspace_ancestors(&self) -> Vec<PathBuf> {
        let current = if self.session_root.is_dir() {
            self.session_root.clone()
        } else {
            self.session_root
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.session_root.clone())
        };
        let mut result = Vec::new();
        for ancestor in current.ancestors() {
            if ancestor != self.home_dir {
                result.push(ancestor.to_path_buf());
            }
        }
        result
    }
}

struct DiscoveryResult {
    documents: BTreeMap<String, SkillDocument>,
    issues: Vec<SkillIssue>,
    scanned_files: u64,
}

pub fn load_skill_document(path: impl AsRef<Path>) -> Result<SkillDocument, String> {
    let skill_path = canonicalize_existing(path.as_ref());
    if !skill_path.is_file() {
        return Err(format!("Skill file not found: {}", skill_path.display()));
    }
    let text = fs::read_to_string(&skill_path).map_err(io_error)?;
    let parsed = parse_frontmatter(&text, &skill_path)?;
    let name = parsed
        .data
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let description = parsed
        .data
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() {
        return Err(format!(
            "Skill file missing required frontmatter field 'name': {}",
            skill_path.display()
        ));
    }
    if description.is_empty() {
        return Err(format!(
            "Skill file missing required frontmatter field 'description': {}",
            skill_path.display()
        ));
    }
    let metadata = parsed
        .data
        .into_iter()
        .filter(|(key, _value)| key != "name" && key != "description")
        .collect::<BTreeMap<_, _>>();
    Ok(SkillDocument {
        name,
        description,
        location: path_to_string(&skill_path),
        directory: path_to_string(skill_path.parent().unwrap_or_else(|| Path::new(""))),
        metadata,
        score: None,
        content: parsed.content,
    })
}

#[must_use]
pub fn render_skill_document(document: &SkillDocument, include_header: bool) -> String {
    let mut lines = Vec::new();
    if include_header {
        lines.extend([
            format!("## Skill: {}", document.name),
            String::new(),
            format!("**Base directory**: {}", document.directory),
            String::new(),
        ]);
    }
    lines.push(document.content.clone());
    lines.join("\n").trim().to_string()
}
