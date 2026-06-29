#[must_use]
pub fn blocked_command(command: &str) -> Option<String> {
    let regex = Regex::new(
        r"(?i)(?:^|[;&|]\s*)(rm|rmdir|del|erase|deltree|remove-item|shred|unlink)(?:\s|$)",
    )
    .ok()?;
    regex
        .captures(command)
        .and_then(|captures| captures.get(1))
        .map(|matched| matched.as_str().to_string())
}

pub fn ensure_within_root(
    root: impl AsRef<Path>,
    target: impl AsRef<Path>,
) -> ToolResultValue<PathBuf> {
    let root_raw = root.as_ref();
    let target_raw = target.as_ref();
    let root_resolved = root_raw
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(root_raw));
    let target_joined = if target_raw.is_absolute() {
        target_raw.to_path_buf()
    } else {
        root_resolved.join(target_raw)
    };
    let target_resolved = target_joined
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(&target_joined));
    if target_resolved == root_resolved || target_resolved.starts_with(&root_resolved) {
        Ok(target_resolved)
    } else {
        Err(format!(
            "Path escapes session root: {}",
            target_raw.display()
        ))
    }
}

pub fn resolve_path_in_root(root: impl AsRef<Path>, path: &str) -> ToolResultValue<PathBuf> {
    ensure_within_root(root, Path::new(path))
}

pub fn resolve_optional_path(
    root: impl AsRef<Path>,
    path: Option<&str>,
) -> ToolResultValue<PathBuf> {
    match path {
        Some(raw) => ensure_within_root(root, Path::new(raw)),
        None => ensure_within_root(&root, root.as_ref()),
    }
}
