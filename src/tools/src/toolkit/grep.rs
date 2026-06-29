#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GrepMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
    pub mtime: f64,
}

fn grep_paths(
    root: &Path,
    base: &Path,
    pattern: &str,
    include_glob: Option<&str>,
) -> ToolResultValue<Vec<GrepMatch>> {
    let regex = Regex::new(pattern).map_err(|error| error.to_string())?;
    let include = include_glob.unwrap_or("*");
    let mut matches = Vec::new();
    for file_path in walk_files(base)? {
        let name = file_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default();
        let relative = file_path
            .strip_prefix(base)
            .map(path_to_string)
            .unwrap_or_else(|_| path_to_string(&file_path));
        if !matches_glob(include, &relative, name) {
            continue;
        }
        let Ok(resolved) = ensure_within_root(root, &file_path) else {
            continue;
        };
        let content = match fs::read_to_string(&resolved) {
            Ok(content) => content,
            Err(_) => continue,
        };
        for (line_index, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                matches.push(GrepMatch {
                    path: path_to_string(&resolved),
                    line: line_index + 1,
                    text: line.to_string(),
                    mtime: path_mtime(&resolved),
                });
            }
        }
    }
    Ok(matches)
}

fn render_grep_output(matches: &[GrepMatch], truncated: bool) -> String {
    if matches.is_empty() {
        return "No files found".to_string();
    }
    let mut output_lines = vec![format!("Found {} matches", matches.len())];
    let mut current_file = String::new();
    for item in matches {
        if current_file != item.path {
            if !current_file.is_empty() {
                output_lines.push(String::new());
            }
            current_file = item.path.clone();
            output_lines.push(format!("{}:", item.path));
        }
        output_lines.push(format!(
            "  Line {}: {}",
            item.line,
            clip_chars(&item.text, MAX_LINE_LENGTH)
        ));
    }
    if truncated {
        output_lines.push(String::new());
        output_lines.push(
            "(Results are truncated. Consider using a more specific path or pattern.)".to_string(),
        );
    }
    output_lines.join("\n")
}
