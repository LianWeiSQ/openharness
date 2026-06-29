fn execute_builtin(name: &str, input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    match name {
        "read" => read_tool(input, ctx),
        "write" => write_tool(input, ctx),
        "edit" => edit_tool(input, ctx),
        "glob" => glob_tool(input, ctx),
        "grep" => grep_tool(input, ctx),
        "ls" => ls_tool(input, ctx),
        "bash" => bash_tool(input, ctx),
        "skill" => skill_tool(input, ctx),
        "code_search" => code_search_tool(input, ctx),
        "memory_read" => memory_read_tool(input, ctx),
        "memory_write" => memory_write_tool(input, ctx),
        "todowrite" => todo_write_tool(input, ctx),
        "todoread" => todo_read_tool(input, ctx),
        "question" => question_tool(input, ctx),
        _ => Err(format!("No Rust builtin implementation for tool: {name}")),
    }
}

fn read_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let file_path = string_arg(&input, "file_path")?;
    let offset = usize_arg(&input, "offset", 0)?;
    let limit = usize_arg(&input, "limit", DEFAULT_READ_LIMIT)?;
    let root = normalize_path(&ctx.session_root);
    let target = resolve_path_in_root(&root, &file_path)?;
    if !target.exists() {
        return Err(format!("File not found: {}", target.display()));
    }
    if target.is_dir() {
        return Err(format!(
            "Path is a directory, not a file: {}",
            target.display()
        ));
    }
    let text = read_text_checked(&target)?;
    let formatted = format_read_output_from_text(&text, offset, limit);
    ctx.remember_read(&target);
    let mut output = ToolOutput::new(display_path(&root, &target), formatted.output);
    output
        .metadata
        .insert("preview".to_string(), json!(formatted.preview));
    output.truncated = formatted.truncated;
    Ok(output)
}

fn write_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let file_path = string_arg(&input, "file_path")?;
    let content = string_arg(&input, "content")?;
    let root = normalize_path(&ctx.session_root);
    let target = resolve_path_in_root(&root, &file_path)?;
    let existed = target.exists();
    require_existing_file_was_read(ctx, &target, "writing")?;
    write_text(&target, &content)?;
    ctx.remember_read(&target);
    let mut output = ToolOutput::new(
        display_path(&root, &target),
        format!(
            "Wrote {} chars to {}",
            content.chars().count(),
            target.display()
        ),
    );
    output
        .metadata
        .insert("file_path".to_string(), json!(path_to_string(&target)));
    output.metadata.insert("exists".to_string(), json!(existed));
    Ok(output)
}

fn edit_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let file_path = string_arg(&input, "file_path")?;
    let old_string = string_arg(&input, "old_string")?;
    let new_string = string_arg(&input, "new_string")?;
    let replace_all = bool_arg(&input, "replace_all", false)?;
    let root = normalize_path(&ctx.session_root);
    let target = resolve_path_in_root(&root, &file_path)?;
    require_existing_file_was_read(ctx, &target, "editing")?;

    if old_string.is_empty() {
        write_text(&target, &new_string)?;
        ctx.remember_read(&target);
        return edited_output(&root, &target, replace_all);
    }
    if !target.exists() {
        return Err(format!("File not found: {}", target.display()));
    }
    if target.is_dir() {
        return Err(format!(
            "Path is a directory, not a file: {}",
            target.display()
        ));
    }
    let text = fs::read_to_string(&target).map_err(io_error)?;
    let new_text = replace_text(&text, &old_string, &new_string, replace_all)?;
    write_text(&target, &new_text)?;
    ctx.remember_read(&target);
    edited_output(&root, &target, replace_all)
}

fn glob_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let pattern = string_arg(&input, "pattern")?;
    let path = optional_string_arg(&input, "path")?;
    let root = normalize_path(&ctx.session_root);
    let base = resolve_optional_path(&root, path.as_deref())?;
    let mut matches = glob_paths(&root, &base, &pattern)?;
    let truncated = matches.len() > GLOB_LIMIT;
    matches.sort_by(|left, right| {
        path_mtime(right)
            .total_cmp(&path_mtime(left))
            .then_with(|| path_to_string(left).cmp(&path_to_string(right)))
    });
    matches.truncate(GLOB_LIMIT);
    let output_text = if matches.is_empty() {
        "No files found".to_string()
    } else {
        let mut lines = matches
            .iter()
            .map(|path| path_to_string(path))
            .collect::<Vec<_>>();
        if truncated {
            lines.push(String::new());
            lines.push(
                "(Results are truncated. Consider using a more specific path or pattern.)"
                    .to_string(),
            );
        }
        lines.join("\n")
    };
    let mut output = ToolOutput::new(display_path(&root, &base), output_text);
    output
        .metadata
        .insert("count".to_string(), json!(matches.len()));
    output.truncated = truncated;
    Ok(output)
}

fn grep_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let pattern = string_arg(&input, "pattern")?;
    let path = optional_string_arg(&input, "path")?;
    let include_glob =
        optional_string_arg(&input, "include")?.or(optional_string_arg(&input, "glob")?);
    let root = normalize_path(&ctx.session_root);
    let base = resolve_optional_path(&root, path.as_deref())?;
    let mut matches = grep_paths(&root, &base, &pattern, include_glob.as_deref())?;
    matches.sort_by(|left, right| {
        right
            .mtime
            .total_cmp(&left.mtime)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
    });
    let truncated = matches.len() > GREP_LIMIT;
    matches.truncate(GREP_LIMIT);
    let output_text = render_grep_output(&matches, truncated);
    let mut output = ToolOutput::new(pattern, output_text);
    output
        .metadata
        .insert("count".to_string(), json!(matches.len()));
    output.metadata.insert(
        "include".to_string(),
        json!(include_glob.unwrap_or_else(|| "*".to_string())),
    );
    output.truncated = truncated;
    Ok(output)
}

fn ls_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let path = optional_string_arg(&input, "path")?;
    let mut ignore = DEFAULT_LS_IGNORE
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    ignore.extend(string_list_arg(&input, "ignore")?);
    let root = normalize_path(&ctx.session_root);
    let base = resolve_optional_path(&root, path.as_deref())?;
    let (tree, count, truncated) = collect_ls_tree(&base, &ignore)?;
    let output_text = render_ls_tree(&format!("{}/", base.display()), &tree, truncated);
    let mut output = ToolOutput::new(display_path(&root, &base), output_text);
    output.metadata.insert("count".to_string(), json!(count));
    output.metadata.insert("ignore".to_string(), json!(ignore));
    output.truncated = truncated;
    Ok(output)
}

fn bash_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let command = string_arg(&input, "command")?;
    let timeout = u64_arg(&input, "timeout", 120_000)?;
    let workdir = optional_string_arg(&input, "workdir")?;
    let description = optional_string_arg(&input, "description")?.unwrap_or_default();
    if let Some(blocked) = blocked_command(&command) {
        return Err(format!(
            "{blocked} command is disabled for security reasons"
        ));
    }
    let runtime = LocalWorkspaceRuntime::new(&ctx.session_root);
    let command_result = runtime.run_command(&command, workdir.as_deref(), timeout)?;
    let combined = format!("{}{}", command_result.stdout, command_result.stderr)
        .trim()
        .to_string();
    let output_text = if combined.is_empty() {
        format!(
            "Command exited with return code {}.",
            command_result.returncode
        )
    } else {
        combined
    };
    let title = display_path(&ctx.session_root, Path::new(&command_result.cwd));
    let mut output = ToolOutput::new(title, output_text);
    output
        .metadata
        .insert("returncode".to_string(), json!(command_result.returncode));
    output
        .metadata
        .insert("description".to_string(), json!(description));
    output
        .metadata
        .insert("workdir".to_string(), json!(command_result.cwd));
    Ok(output)
}

fn skill_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let requested_name = optional_string_arg(&input, "name")?
        .unwrap_or_default()
        .trim()
        .to_string();
    let requested_query = optional_string_arg(&input, "query")?
        .unwrap_or_default()
        .trim()
        .to_string();
    let limit = optional_usize_arg(&input, "limit")?.filter(|value| *value > 0);
    let include_content = bool_arg(&input, "include_content", false)?;
    let include_diagnostics = bool_arg(&input, "include_diagnostics", false)?;
    let registry = SkillRegistry::new(
        Some(ctx.session_root.clone()),
        skill_roots(ctx),
        Option::<PathBuf>::None,
    );

    if requested_name.is_empty() {
        let report = registry.report(
            (!requested_query.is_empty()).then_some(requested_query.as_str()),
            limit,
        );
        let mut lines = if report.skills.is_empty() {
            if requested_query.is_empty() {
                vec!["No skills available.".to_string()]
            } else {
                vec![format!("No skills matched query \"{requested_query}\".")]
            }
        } else {
            let mut lines = vec![if requested_query.is_empty() {
                "Available skills:".to_string()
            } else {
                format!("Matched skills for \"{requested_query}\":")
            }];
            for skill in &report.skills {
                let score = skill
                    .score
                    .map(|score| format!(" score={score}"))
                    .unwrap_or_default();
                lines.push(format!(
                    "- `{}`:{} {}",
                    skill.name, score, skill.description
                ));
                if include_content && let Some(document) = registry.get(&skill.name) {
                    lines.push(render_skill_document(&document, false));
                }
            }
            lines
        };
        if include_diagnostics {
            lines.extend(diagnostic_lines(&report));
        }
        let mut output = ToolOutput::new("Available skills", lines.join("\n"));
        output.metadata = report_metadata(
            &report,
            (!requested_query.is_empty()).then_some(requested_query),
        );
        return Ok(output);
    }

    let Some(document) = registry.get(&requested_name) else {
        let skills = if requested_query.is_empty() {
            registry.all()
        } else {
            registry.search(&requested_query, limit)
        };
        let available = if skills.is_empty() {
            "none".to_string()
        } else {
            skills
                .iter()
                .map(|skill| skill.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(format!(
            "Skill \"{requested_name}\" not found. Available skills: {available}"
        ));
    };

    let report = registry.report(None, None);
    let mut output = ToolOutput::new(
        format!("Loaded skill: {}", document.name),
        render_skill_document(&document, true),
    );
    output
        .metadata
        .insert("skill_name".to_string(), json!(document.name));
    output
        .metadata
        .insert("skill_location".to_string(), json!(document.location));
    output
        .metadata
        .insert("skill_dir".to_string(), json!(document.directory));
    output
        .metadata
        .insert("skill_count".to_string(), json!(report.loaded_count));
    output
        .metadata
        .insert("scanned_files".to_string(), json!(report.scanned_files));
    output
        .metadata
        .insert("invalid_count".to_string(), json!(report.invalid_count));
    output
        .metadata
        .insert("duplicate_count".to_string(), json!(report.duplicate_count));
    Ok(output)
}

fn code_search_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let query = string_arg(&input, "query")?;
    let glob = string_arg_or(&input, "glob", "*")?;
    let path = optional_string_arg(&input, "path")?;
    let root = normalize_path(&ctx.session_root);
    let base = resolve_optional_path(&root, path.as_deref())?;
    let mut hits = Vec::new();
    let mut preview_hits = Vec::new();
    for file_path in walk_files(&base)? {
        let name = file_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default();
        let relative = file_path
            .strip_prefix(&base)
            .map(path_to_string)
            .unwrap_or_else(|_| path_to_string(&file_path));
        if !matches_glob(&glob, &relative, name) {
            continue;
        }
        let content = match fs::read_to_string(&file_path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        for (index, line) in content.lines().enumerate() {
            if !line.contains(&query) {
                continue;
            }
            let clipped = clip_chars(line, CODE_SEARCH_MAX_LINE_CHARS);
            let hit = format!("{}:{}:{clipped}", file_path.display(), index + 1);
            hits.push(hit.clone());
            if preview_hits.len() < CODE_SEARCH_MAX_PREVIEW_HITS {
                preview_hits.push(hit);
            }
            if hits.len() >= CODE_SEARCH_MAX_HITS {
                return code_search_output(&root, &base, hits, preview_hits, true);
            }
        }
    }
    code_search_output(&root, &base, hits, preview_hits, false)
}

fn memory_read_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let key = string_arg(&input, "key")?;
    let value = ctx.memory.get(&key).cloned().unwrap_or(Value::Null);
    Ok(ToolOutput::new(key, value.to_string()))
}

fn memory_write_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let key = string_arg(&input, "key")?;
    let value = input
        .get("value")
        .cloned()
        .ok_or_else(|| "Missing required parameter: value".to_string())?;
    ctx.memory.insert(key.clone(), value);
    Ok(ToolOutput::new(key, "ok"))
}

fn todo_write_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let todos_value = input
        .get("todos")
        .and_then(Value::as_array)
        .ok_or_else(|| "Expected list input for todos".to_string())?;
    let todos = todos_value
        .iter()
        .map(todo_from_value)
        .collect::<ToolResultValue<Vec<_>>>()?;
    save_todos(ctx, todos.clone())?;
    todo_output(todos)
}

fn todo_read_tool(_input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let todos = load_todos(ctx)?;
    todo_output(todos)
}

fn question_tool(input: Value, ctx: &mut ToolContext) -> ToolResultValue<ToolOutput> {
    let questions = input
        .get("questions")
        .and_then(Value::as_array)
        .ok_or_else(|| "Expected list input for questions".to_string())?;
    let answers = ctx
        .question_answers
        .clone()
        .ok_or_else(|| "question tool requires configured answers".to_string())?;
    let mut formatted = Vec::new();
    let mut question_metadata = Vec::new();
    for (index, item) in questions.iter().enumerate() {
        let question = value_string(item, "question")?;
        let answer = answers
            .get(index)
            .map(|items| {
                if items.is_empty() {
                    "Unanswered".to_string()
                } else {
                    items.join(", ")
                }
            })
            .unwrap_or_else(|| "Unanswered".to_string());
        formatted.push(format!("\"{question}\"=\"{answer}\""));
        question_metadata.push(question_metadata_value(item)?);
    }
    let count = questions.len();
    let mut output = ToolOutput::new(
        title_for_questions(count),
        format!(
            "User has answered your questions: {}. You can now continue with the user's answers in mind.",
            formatted.join(", ")
        ),
    );
    output
        .metadata
        .insert("answers".to_string(), json!(answers));
    output
        .metadata
        .insert("questions".to_string(), Value::Array(question_metadata));
    output.metadata.insert(
        "request_id".to_string(),
        json!(format!("question_{}", ctx.call_id)),
    );
    output.metadata.insert("count".to_string(), json!(count));
    Ok(output)
}
