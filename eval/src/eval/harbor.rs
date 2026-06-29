#[must_use]
pub fn harbor_timeout_seconds(timeout_ms: i64) -> i64 {
    let seconds = (timeout_ms as f64 / 1000.0).ceil() as i64;
    seconds.max(1)
}

#[must_use]
pub fn harbor_success_command(spec: HarborSuccessSpec<'_>) -> (HarborCommandRecord, CommandResult) {
    let resolved_cwd = spec.cwd.unwrap_or(spec.workspace_root).to_string();
    let command_record = HarborCommandRecord {
        command: spec.command.to_string(),
        cwd: resolved_cwd.clone(),
        timeout_sec: harbor_timeout_seconds(spec.timeout_ms),
    };
    let suffix = format!(
        "[openagent harbor] exit_code={} duration_ms={}",
        spec.returncode, spec.elapsed_ms
    );
    let formatted_stdout = if spec.stdout.trim().is_empty() {
        suffix
    } else {
        format!("{}\n{suffix}", spec.stdout.trim_end())
    };
    (
        command_record,
        CommandResult {
            cwd: resolved_cwd,
            returncode: spec.returncode,
            stderr: spec.stderr.to_string(),
            stdout: formatted_stdout,
        },
    )
}

#[must_use]
pub fn harbor_timeout_command(
    command: &str,
    cwd: Option<&str>,
    timeout_ms: i64,
    workspace_root: &str,
    elapsed_ms: i64,
    error: &str,
) -> (HarborCommandRecord, CommandResult) {
    let resolved_cwd = cwd.unwrap_or(workspace_root).to_string();
    let command_record = HarborCommandRecord {
        command: command.to_string(),
        cwd: resolved_cwd.clone(),
        timeout_sec: harbor_timeout_seconds(timeout_ms),
    };
    (
        command_record,
        CommandResult {
            cwd: resolved_cwd,
            returncode: 124,
            stderr: error.to_string(),
            stdout: format!("[openagent harbor] exit_code=124 duration_ms={elapsed_ms}"),
        },
    )
}

#[must_use]
pub fn harbor_normalized_model_name(value: Option<&str>) -> Option<String> {
    let raw = value.unwrap_or("").trim();
    if raw.is_empty() {
        return None;
    }
    let Some((provider, model_name)) = raw.split_once('/') else {
        return Some(raw.to_string());
    };
    if matches!(
        provider.to_ascii_lowercase().as_str(),
        "openai" | "openai-compatible"
    ) {
        Some(model_name.to_string())
    } else {
        Some(raw.to_string())
    }
}

#[must_use]
pub fn harbor_system_prompt(workspace_root: &str) -> String {
    format!(
        "You are OpenAgent running inside Terminal-Bench 2.0 through Harbor. Complete the task by using only the bash tool.\n\
The bash tool executes commands in the benchmark environment. The default workspace is {workspace_root}.\n\
Each tool call can pass an explicit workdir; otherwise it runs in the default workspace.\n\
Inspect the environment, modify files with shell commands when needed, run validation commands, and iterate from failures.\n\
Do not ask the user questions. When the task is complete, provide a concise final answer."
    )
}
