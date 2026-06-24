use std::{fs, path::Path};

pub(crate) fn initialize_openagent_project_files(workspace: &Path) -> Result<Vec<String>, String> {
    let mut created = Vec::new();
    let agents_path = workspace.join("AGENTS.md");
    if !agents_path.exists() {
        fs::write(
            &agents_path,
            "# OpenAgent Project Guide\n\n- Use the existing project style.\n- Run focused tests after meaningful changes.\n- Keep unrelated worktree changes intact.\n",
        )
        .map_err(|error| error.to_string())?;
        created.push("AGENTS.md".to_string());
    }
    let config_dir = workspace.join(".openagent");
    fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    let tui_config = config_dir.join("tui.jsonc");
    if !tui_config.exists() {
        fs::write(
            &tui_config,
            "{\n  // OpenAgent TUI settings\n  \"theme\": \"default\",\n  \"color_scheme\": \"system\",\n  \"leader_key\": \"\\\\\",\n  \"mouse\": true,\n  \"scroll\": 5,\n  \"diff_style\": \"unified\",\n  \"attention_notifications\": true,\n  \"sounds\": false,\n  \"keybinds\": {\n    \"editor\": \"ctrl+e\",\n    \"stash\": \"ctrl+s\",\n    \"unstash\": \"ctrl+y\"\n  }\n}\n",
        )
        .map_err(|error| error.to_string())?;
        created.push(".openagent/tui.jsonc".to_string());
    }
    if created.is_empty() {
        created.push("already up to date".to_string());
    }
    Ok(created)
}
