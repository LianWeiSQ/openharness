use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Style};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TuiConfig {
    pub theme: String,
    pub color_scheme: String,
    pub keybinds: BTreeMap<String, String>,
    pub leader_key: String,
    pub mouse: bool,
    pub scroll: u16,
    pub diff_style: String,
    pub attention_notifications: bool,
    pub sounds: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),
            color_scheme: "system".to_string(),
            keybinds: BTreeMap::from([
                ("editor".to_string(), "ctrl+e".to_string()),
                ("stash".to_string(), "ctrl+s".to_string()),
                ("unstash".to_string(), "ctrl+y".to_string()),
            ]),
            leader_key: "\\".to_string(),
            mouse: true,
            scroll: 5,
            diff_style: "unified".to_string(),
            attention_notifications: true,
            sounds: false,
        }
    }
}

impl TuiConfig {
    #[must_use]
    pub fn load_from_workspace(workspace: &Path) -> Self {
        let mut config = Self::default();
        let Some(path) = find_tui_config_path(workspace) else {
            return config;
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return config;
        };
        let parsed = serde_json::from_str::<Value>(&strip_jsonc_comments(&raw))
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        if let Some(theme) = parsed.get("theme").and_then(Value::as_str) {
            config.theme = theme.to_string();
        }
        if let Some(color_scheme) = parsed
            .get("color_scheme")
            .or_else(|| parsed.get("scheme"))
            .and_then(Value::as_str)
            .filter(|value| is_valid_color_scheme(value))
        {
            config.color_scheme = color_scheme.to_string();
        }
        if let Some(keybinds) = parsed.get("keybinds").and_then(Value::as_object) {
            for (key, value) in keybinds {
                if let Some(binding) = value.as_str().filter(|value| !value.trim().is_empty()) {
                    config
                        .keybinds
                        .insert(key.to_string(), binding.trim().to_ascii_lowercase());
                }
            }
        }
        if let Some(leader_key) = parsed.get("leader_key").and_then(Value::as_str) {
            config.leader_key = leader_key.to_string();
        }
        if let Some(mouse) = parsed.get("mouse").and_then(Value::as_bool) {
            config.mouse = mouse;
        }
        if let Some(scroll) = parsed.get("scroll").and_then(Value::as_u64) {
            config.scroll = scroll.min(100) as u16;
        }
        if let Some(diff_style) = parsed.get("diff_style").and_then(Value::as_str) {
            config.diff_style = diff_style.to_string();
        }
        if let Some(enabled) = parsed
            .get("attention_notifications")
            .or_else(|| parsed.get("notifications"))
            .and_then(Value::as_bool)
        {
            config.attention_notifications = enabled;
        }
        if let Some(sounds) = parsed.get("sounds").and_then(Value::as_bool) {
            config.sounds = sounds;
        }
        config
    }
}

pub(crate) fn keybind_matches(
    config: &TuiConfig,
    key: &KeyEvent,
    action: &str,
    default: &str,
) -> bool {
    let binding = config
        .keybinds
        .get(action)
        .map(String::as_str)
        .unwrap_or(default);
    key_matches_binding(key, binding)
}

fn key_matches_binding(key: &KeyEvent, binding: &str) -> bool {
    let normalized = binding.trim().to_ascii_lowercase();
    if let Some(value) = normalized.strip_prefix("ctrl+") {
        return key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char(ch) if ch.to_ascii_lowercase().to_string() == value);
    }
    if let Some(value) = normalized.strip_prefix("leader+") {
        return matches!(key.code, KeyCode::Char(ch) if ch.to_ascii_lowercase().to_string() == value);
    }
    match normalized.as_str() {
        "esc" | "escape" => key.code == KeyCode::Esc,
        "enter" => key.code == KeyCode::Enter,
        "up" => key.code == KeyCode::Up,
        "down" => key.code == KeyCode::Down,
        value if value.len() == 1 => {
            matches!(key.code, KeyCode::Char(ch) if ch.to_ascii_lowercase().to_string() == value)
        }
        _ => false,
    }
}

pub(crate) fn default_theme_names() -> Vec<String> {
    ["default", "light", "high-contrast", "midnight"]
        .into_iter()
        .map(ToString::to_string)
        .collect()
}

pub(crate) fn default_color_scheme_names() -> Vec<String> {
    ["system", "light", "dark"]
        .into_iter()
        .map(ToString::to_string)
        .collect()
}

pub(crate) fn is_valid_color_scheme(value: &str) -> bool {
    matches!(value.trim(), "system" | "light" | "dark")
}

pub(crate) fn string_array(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

pub(crate) fn timeline_style(config: &TuiConfig, kind: &str) -> Style {
    let color = match config.theme.as_str() {
        "light" => match kind {
            "error" => Color::Red,
            "warning" => Color::Yellow,
            "user" => Color::Blue,
            "assistant" => Color::Black,
            "patch" | "diff-hunk" => Color::Blue,
            "diff-add" => Color::Green,
            "diff-del" => Color::Red,
            "diff-meta" => Color::DarkGray,
            _ => Color::DarkGray,
        },
        "high-contrast" => match kind {
            "error" => Color::LightRed,
            "warning" => Color::LightYellow,
            "user" => Color::LightGreen,
            "assistant" => Color::White,
            "patch" | "diff-hunk" => Color::LightCyan,
            "diff-add" => Color::LightGreen,
            "diff-del" => Color::LightRed,
            "diff-meta" => Color::LightYellow,
            _ => Color::LightCyan,
        },
        "midnight" => match kind {
            "error" => Color::LightRed,
            "warning" => Color::LightMagenta,
            "user" => Color::LightBlue,
            "assistant" => Color::White,
            "patch" | "diff-hunk" => Color::LightBlue,
            "diff-add" => Color::LightGreen,
            "diff-del" => Color::LightRed,
            "diff-meta" => Color::DarkGray,
            _ => Color::Cyan,
        },
        _ => match kind {
            "error" => Color::Red,
            "warning" => Color::Yellow,
            "user" => Color::Green,
            "assistant" => Color::White,
            "patch" | "diff-hunk" => Color::Cyan,
            "diff-add" => Color::Green,
            "diff-del" => Color::Red,
            "diff-meta" => Color::DarkGray,
            _ => Color::Gray,
        },
    };
    Style::default().fg(color)
}

fn find_tui_config_path(workspace: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("OPENAGENT_TUI_CONFIG")
        && !path.trim().is_empty()
    {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    [
        workspace.join(".openagent/tui.jsonc"),
        workspace.join(".openagent/tui.json"),
        workspace.join("tui.jsonc"),
        workspace.join("tui.json"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn strip_jsonc_comments(raw: &str) -> String {
    let mut output = String::new();
    let mut chars = raw.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for next in chars.by_ref() {
                if next == '\n' {
                    output.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = '\0';
            for next in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
            continue;
        }
        output.push(ch);
    }
    output
}
