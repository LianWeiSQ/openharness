#[derive(Clone, Debug, Default)]
struct LsNode {
    dirs: BTreeMap<String, LsNode>,
    files: Vec<String>,
}

fn collect_ls_tree(base: &Path, ignore: &[String]) -> ToolResultValue<(LsNode, usize, bool)> {
    let mut root = LsNode::default();
    if base.is_file() {
        root.files.push(
            base.file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_default()
                .to_string(),
        );
        return Ok((root, 1, false));
    }
    let mut file_count = 0usize;
    let mut truncated = false;
    collect_ls_tree_inner(
        base,
        base,
        ignore,
        &mut root,
        &mut file_count,
        &mut truncated,
    )?;
    Ok((root, file_count, truncated))
}

fn collect_ls_tree_inner(
    base: &Path,
    dir: &Path,
    ignore: &[String],
    node: &mut LsNode,
    file_count: &mut usize,
    truncated: &mut bool,
) -> ToolResultValue<()> {
    if *truncated || !dir.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(dir)
        .map_err(io_error)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries.iter().filter(|path| path.is_dir()) {
        let relative = path
            .strip_prefix(base)
            .map(path_to_string)
            .unwrap_or_default();
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        if should_ignore(&relative, name, ignore) {
            continue;
        }
        let child = node.dirs.entry(name.to_string()).or_default();
        collect_ls_tree_inner(base, path, ignore, child, file_count, truncated)?;
        if *truncated {
            return Ok(());
        }
    }
    for path in entries.into_iter().filter(|path| path.is_file()) {
        let relative = path
            .strip_prefix(base)
            .map(path_to_string)
            .unwrap_or_default();
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        if should_ignore(&relative, name, ignore) {
            continue;
        }
        node.files.push(name.to_string());
        *file_count += 1;
        if *file_count >= LS_LIMIT {
            *truncated = true;
            return Ok(());
        }
    }
    Ok(())
}

fn render_ls_tree(label: &str, tree: &LsNode, truncated: bool) -> String {
    let mut lines = vec![label.to_string()];
    render_ls_tree_inner(tree, 0, &mut lines);
    if truncated {
        lines.push(String::new());
        lines.push("(Results are truncated. Consider using a more specific path.)".to_string());
    }
    lines.join("\n")
}

fn render_ls_tree_inner(node: &LsNode, depth: usize, lines: &mut Vec<String>) {
    for (dirname, child) in &node.dirs {
        lines.push(format!("{}{}{}", "  ".repeat(depth + 1), dirname, "/"));
        render_ls_tree_inner(child, depth + 1, lines);
    }
    let mut files = node.files.clone();
    files.sort();
    for filename in files {
        lines.push(format!("{}{}", "  ".repeat(depth + 1), filename));
    }
}

fn should_ignore(relative_path: &str, name: &str, patterns: &[String]) -> bool {
    let normalized = relative_path.replace('\\', "/");
    patterns.iter().any(|pattern| {
        let cleaned = pattern.replace('\\', "/");
        if cleaned.ends_with('/') {
            let prefix = cleaned.trim_end_matches('/');
            normalized == prefix || normalized.starts_with(&format!("{prefix}/")) || name == prefix
        } else {
            matches_glob(&cleaned, &normalized, name)
        }
    })
}

fn matches_glob(pattern: &str, relative: &str, name: &str) -> bool {
    let target = if pattern.contains('/') {
        relative
    } else {
        name
    };
    let regex_source = format!("^{}$", glob_to_regex(pattern));
    Regex::new(&regex_source)
        .map(|regex| regex.is_match(target))
        .unwrap_or(false)
}

fn glob_to_regex(pattern: &str) -> String {
    let mut regex = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' if chars.peek() == Some(&'*') => {
                let _ = chars.next();
                regex.push_str(".*");
            }
            '*' => regex.push_str("[^/]*"),
            '?' => regex.push_str("[^/]"),
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '[' | ']' | '{' | '}' | '\\' => {
                regex.push('\\');
                regex.push(ch);
            }
            other => regex.push(other),
        }
    }
    regex
}

fn path_mtime(path: &Path) -> f64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}
