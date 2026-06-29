fn normalize_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn root_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| normalize_path(path))
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn display_path(root: &Path, target: &Path) -> String {
    normalize_path(target)
        .strip_prefix(normalize_path(root))
        .map(path_to_string)
        .unwrap_or_else(|_| path_to_string(&normalize_path(target)))
}

fn io_error(error: io::Error) -> String {
    error.to_string()
}

fn read_text_checked(path: &Path) -> ToolResultValue<String> {
    if is_binary_file(path)? {
        return Err(format!("Cannot read binary file: {}", path.display()));
    }
    fs::read_to_string(path).map_err(io_error)
}

fn is_binary_file(path: &Path) -> ToolResultValue<bool> {
    let suffix = path
        .extension()
        .and_then(OsStr::to_str)
        .map(|extension| format!(".{}", extension.to_lowercase()))
        .unwrap_or_default();
    if BINARY_EXTENSIONS.contains(&suffix.as_str()) {
        return Ok(true);
    }
    let data = fs::read(path).map_err(io_error)?;
    if data.is_empty() {
        return Ok(false);
    }
    let sample = data.iter().take(4096).copied().collect::<Vec<_>>();
    if sample.contains(&0) {
        return Ok(true);
    }
    let non_printable = sample
        .iter()
        .filter(|byte| **byte < 9 || (**byte > 13 && **byte < 32))
        .count();
    Ok((non_printable as f64 / sample.len() as f64) > 0.3)
}

fn write_text(path: &Path, content: &str) -> ToolResultValue<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    fs::write(path, content).map_err(io_error)
}

fn require_existing_file_was_read(
    ctx: &ToolContext,
    target: &Path,
    action: &str,
) -> ToolResultValue<()> {
    if ctx.require_read_before_write && target.exists() && !ctx.has_read_file(target) {
        Err(format!(
            "Must read existing file before {action} it: {}",
            target.display()
        ))
    } else {
        Ok(())
    }
}

fn replace_text(content: &str, old: &str, new: &str, replace_all: bool) -> ToolResultValue<String> {
    if old == new {
        return Err("old_string and new_string must be different".to_string());
    }
    if old.is_empty() {
        return Ok(new.to_string());
    }
    let count = content.matches(old).count();
    if count == 0 {
        return Err("old_string not found in content".to_string());
    }
    if count > 1 && !replace_all {
        return Err(
            "old_string found multiple times and requires more code context to uniquely identify the intended match"
                .to_string(),
        );
    }
    if replace_all {
        Ok(content.replace(old, new))
    } else {
        Ok(content.replacen(old, new, 1))
    }
}

fn edited_output(root: &Path, target: &Path, replace_all: bool) -> ToolResultValue<ToolOutput> {
    let mut output = ToolOutput::new(
        display_path(root, target),
        format!("Edited {}", target.display()),
    );
    output
        .metadata
        .insert("file_path".to_string(), json!(path_to_string(target)));
    output
        .metadata
        .insert("replace_all".to_string(), json!(replace_all));
    Ok(output)
}

fn walk_paths(base: &Path) -> ToolResultValue<Vec<PathBuf>> {
    let mut paths = Vec::new();
    if !base.exists() {
        return Ok(paths);
    }
    paths.push(base.to_path_buf());
    if base.is_dir() {
        let mut entries = fs::read_dir(base)
            .map_err(io_error)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                paths.extend(walk_paths(&path)?);
            } else {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

fn walk_files(base: &Path) -> ToolResultValue<Vec<PathBuf>> {
    Ok(walk_paths(base)?
        .into_iter()
        .filter(|path| path.is_file())
        .collect())
}

fn glob_paths(root: &Path, base: &Path, pattern: &str) -> ToolResultValue<Vec<PathBuf>> {
    let mut matches = BTreeSet::new();
    for path in walk_paths(base)? {
        let relative = path
            .strip_prefix(base)
            .map(path_to_string)
            .unwrap_or_else(|_| path_to_string(&path));
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        if matches_glob(pattern, &relative, name)
            && let Ok(resolved) = ensure_within_root(root, &path)
        {
            matches.insert(resolved);
        }
    }
    Ok(matches.into_iter().collect())
}
