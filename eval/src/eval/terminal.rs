#[must_use]
pub fn execution_metadata(mode: &str, workspace_root: &str, harness: &str) -> Value {
    json!({
        "execution_mode": mode,
        "workspace_root": workspace_root,
        "harness": harness,
    })
}

#[must_use]
pub fn display_path(workspace_root: &str, path: &str) -> String {
    let root = workspace_root.trim_end_matches('/');
    if path == root {
        return ".".to_string();
    }
    let prefix = format!("{root}/");
    path.strip_prefix(&prefix)
        .map_or_else(|| path.to_string(), ToString::to_string)
}

#[must_use]
pub fn terminal_bench_wrap_command(command: &str, cwd: Option<&str>, marker: &str) -> String {
    let mut lines = vec!["set +e".to_string()];
    if let Some(cwd) = cwd {
        lines.push(format!("cd {}", shell_quote(cwd)));
    }
    lines.extend([
        "(".to_string(),
        command.to_string(),
        ")".to_string(),
        "status=$?".to_string(),
        format!(
            "printf {} \"$status\"",
            shell_quote(&format!("\\n{marker}%s\\n"))
        ),
    ]);
    format!("bash -lc {}", shell_quote(&lines.join("\n")))
}

#[must_use]
pub fn terminal_bench_extract_returncode(observation: &str, marker: &str) -> (i64, String) {
    let pattern = format!(r"{}(?P<code>-?\d+)", regex::escape(marker));
    let Ok(regex) = Regex::new(&pattern) else {
        return (0, observation.to_string());
    };
    let code = regex
        .captures_iter(observation)
        .filter_map(|captures| captures.name("code"))
        .filter_map(|code| code.as_str().parse::<i64>().ok())
        .last();
    match code {
        Some(returncode) => (
            returncode,
            regex.replace_all(observation, "").trim().to_string(),
        ),
        None => (0, observation.to_string()),
    }
}

#[must_use]
pub fn terminal_bench_format_observation(
    observation: &str,
    returncode: i64,
    elapsed_ms: i64,
) -> String {
    let body = observation.trim();
    let suffix =
        format!("[openagent terminal-bench] exit_code={returncode} duration_ms={elapsed_ms}");
    if body.is_empty() {
        suffix
    } else {
        format!("{body}\n{suffix}")
    }
}

#[must_use]
pub fn terminal_bench_failure_mode(message: &str) -> &'static str {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("timeout") {
        "agent_timeout"
    } else if lowered.contains("context") && lowered.contains("length") {
        "context_length_exceeded"
    } else if lowered.contains("output") && lowered.contains("length") {
        "output_length_exceeded"
    } else {
        "unknown_agent_error"
    }
}

#[must_use]
pub fn terminal_bench_system_prompt(workspace_root: &str) -> String {
    format!(
        "You are OpenAgent running inside Terminal-Bench. Complete the task by using only the bash tool.\n\
The bash tool executes commands in the benchmark tmux session. The default workspace is {workspace_root}.\n\
Directory changes do not persist between tool calls, so use absolute paths or combine commands with `cd <dir> && ...`.\n\
Inspect the environment, modify files with shell commands when needed, run validation commands, and iterate from failures.\n\
Do not ask the user questions. When the task is complete, provide a concise final answer."
    )
}
