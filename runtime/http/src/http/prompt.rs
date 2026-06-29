#[must_use]
pub fn build_run_prompt(message: &str, files: &[(&str, &str)]) -> String {
    let mut parts = Vec::new();
    if !message.trim().is_empty() {
        parts.push(message.trim().to_string());
    }
    for (path, content) in files {
        parts.push(format!("Attached file: {path}\n\n```text\n{content}\n```"));
    }
    parts.join("\n\n").trim().to_string()
}

#[must_use]
pub fn command_text_from_args(message: &[&str], stdin: Option<&str>, stdin_is_tty: bool) -> String {
    let message = message.join(" ").trim().to_string();
    if !message.is_empty() {
        return message;
    }
    if stdin_is_tty {
        return String::new();
    }
    stdin.unwrap_or_default().trim().to_string()
}
