use std::{fs, io, process::Command};

use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use serde_json::json;

use crate::{
    TimelineLine, TuiState,
    attachments::normalize_attachment_reference_token,
    commands::BUILTIN_COMMANDS,
    config::{default_color_scheme_names, is_valid_color_scheme},
    util::{clip_chars, compact_json},
};

impl TuiState {
    pub fn remember_history(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        if self.history.last().is_some_and(|last| last == value) {
            self.history_cursor = None;
            return;
        }
        self.history.push(value.to_string());
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.history_cursor = None;
    }

    pub fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let index = self.history_cursor.map_or_else(
            || self.history.len().saturating_sub(1),
            |index| index.saturating_sub(1),
        );
        self.history_cursor = Some(index);
        self.input_buffer = self.history[index].clone();
        self.status = "history".to_string();
    }

    pub fn history_next(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.history_cursor = None;
            self.input_buffer.clear();
        } else {
            let next = index + 1;
            self.history_cursor = Some(next);
            self.input_buffer = self.history[next].clone();
        }
        self.status = "history".to_string();
    }

    pub fn stash_current_input(&mut self) {
        let value = self.input_buffer.trim().to_string();
        if value.is_empty() {
            self.timeline
                .push(TimelineLine::new("warning", "nothing to stash", true));
            self.status = "stash empty".to_string();
            return;
        }
        self.stash.push(value);
        if self.stash.len() > 20 {
            self.stash.remove(0);
        }
        self.input_buffer.clear();
        self.timeline.push(TimelineLine::new(
            "status",
            format!("stashed draft: {} item(s)", self.stash.len()),
            true,
        ));
        self.status = "stashed".to_string();
    }

    pub fn restore_latest_stash(&mut self) {
        let Some(value) = self.stash.pop() else {
            self.timeline
                .push(TimelineLine::new("warning", "stash is empty", true));
            self.status = "stash empty".to_string();
            return;
        };
        self.input_buffer = value;
        self.status = "stash restored".to_string();
    }

    pub fn insert_attachment_reference(&mut self, reference: &str) {
        let Some(token) = normalize_attachment_reference_token(reference) else {
            self.timeline.push(TimelineLine::new(
                "warning",
                "usage: /attach <path[:range]>",
                true,
            ));
            self.status = "attach invalid".to_string();
            return;
        };
        if !self.input_buffer.is_empty()
            && !self
                .input_buffer
                .chars()
                .last()
                .is_some_and(char::is_whitespace)
        {
            self.input_buffer.push(' ');
        }
        self.input_buffer.push_str(&token);
        self.input_buffer.push(' ');
        self.timeline.push(TimelineLine::new(
            "status",
            format!("attached reference inserted: {token}"),
            true,
        ));
        self.status = "attachment inserted".to_string();
    }

    pub fn set_theme(&mut self, theme: &str) {
        self.config.theme = theme.to_string();
        self.timeline.push(TimelineLine::new(
            "status",
            format!("theme set to {}", self.config.theme),
            true,
        ));
        self.status = "theme updated".to_string();
    }

    pub fn set_color_scheme(&mut self, scheme: &str) -> bool {
        let scheme = scheme.trim();
        if !is_valid_color_scheme(scheme) {
            self.timeline.push(TimelineLine::new(
                "warning",
                "usage: /theme-scheme <system|light|dark>",
                true,
            ));
            self.status = "color scheme invalid".to_string();
            return false;
        }
        self.config.color_scheme = scheme.to_string();
        self.timeline.push(TimelineLine::new(
            "status",
            format!("color scheme set to {}", self.config.color_scheme),
            true,
        ));
        self.status = "color scheme updated".to_string();
        true
    }

    pub fn cycle_color_scheme(&mut self) {
        let schemes = default_color_scheme_names();
        let current = schemes
            .iter()
            .position(|scheme| scheme == &self.config.color_scheme)
            .unwrap_or(0);
        let next = schemes[(current + 1) % schemes.len()].clone();
        self.set_color_scheme(&next);
    }

    pub fn edit_input_with_external_editor(&mut self) -> Result<(), String> {
        let editor = std::env::var("VISUAL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var("EDITOR")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            })
            .unwrap_or_else(|| "vi".to_string());
        let path = std::env::temp_dir().join(format!(
            "openagent-tui-editor-{}-{}.md",
            std::process::id(),
            self.history.len()
        ));
        fs::write(&path, &self.input_buffer).map_err(|error| error.to_string())?;
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let status = Command::new(&editor)
            .arg(&path)
            .status()
            .map_err(|error| format!("failed to launch editor `{editor}`: {error}"));
        let _ = execute!(io::stdout(), EnterAlternateScreen);
        let _ = enable_raw_mode();
        status.and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!("editor exited with status {status}"))
            }
        })?;
        self.input_buffer = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let _ = fs::remove_file(&path);
        self.status = "editor updated prompt".to_string();
        Ok(())
    }

    pub fn submit(&mut self) -> bool {
        let raw_text = self.input_buffer.trim().to_string();
        if raw_text == "/help" || raw_text == "/?" || raw_text == "/" {
            self.show_help();
            self.input_buffer.clear();
            return false;
        }
        if self.handle_interaction_command(&raw_text) {
            if !raw_text.strip_prefix('/').is_some_and(|command| {
                command
                    .split_whitespace()
                    .next()
                    .is_some_and(|name| matches!(name, "unstash" | "editor" | "attach"))
            }) {
                self.input_buffer.clear();
            }
            return false;
        }
        if raw_text.is_empty() {
            return false;
        }
        self.input_buffer.clear();
        self.status = "running".to_string();
        self.timeline
            .push(TimelineLine::new("user", format!("> {raw_text}"), true));
        true
    }

    pub(crate) fn show_help(&mut self) {
        let lines = BUILTIN_COMMANDS
            .iter()
            .map(|(name, description)| format!("{name} - {description}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.timeline.push(TimelineLine::new(
            "status",
            format!("built-in commands:\n{lines}"),
            true,
        ));
        self.status = "help listed".to_string();
    }

    fn handle_interaction_command(&mut self, raw_text: &str) -> bool {
        let Some(command_line) = raw_text.strip_prefix('/') else {
            return false;
        };
        let mut parts = command_line.split_whitespace();
        let Some(name) = parts.next() else {
            return false;
        };
        match name {
            "allow" | "approve" => {
                let scope = parts.next();
                self.respond_active_approval("allow", scope, None);
                true
            }
            "deny" | "reject" => {
                let note = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                self.respond_active_approval("deny", None, note);
                true
            }
            "answer" => {
                let answer = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                if answer.is_empty() {
                    self.status = "question invalid".to_string();
                    self.timeline
                        .push(TimelineLine::new("warning", "usage: /answer <text>", true));
                    return true;
                }
                self.answer_active_question(vec![vec![answer.to_string()]], None);
                true
            }
            "dismiss" => {
                let note = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                self.dismiss_active_question(note);
                true
            }
            "stash" => {
                let draft = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                if draft.is_empty() {
                    self.timeline
                        .push(TimelineLine::new("warning", "usage: /stash <draft>", true));
                    self.status = "stash empty".to_string();
                } else {
                    self.input_buffer = draft.to_string();
                    self.stash_current_input();
                }
                true
            }
            "unstash" => {
                self.restore_latest_stash();
                true
            }
            "stashes" => {
                if self.stash.is_empty() {
                    self.timeline
                        .push(TimelineLine::new("status", "stash: empty", false));
                } else {
                    self.timeline.push(TimelineLine::new(
                        "status",
                        format!("stash: {} item(s)", self.stash.len()),
                        false,
                    ));
                    for (index, item) in self.stash.iter().rev().take(10).enumerate() {
                        self.timeline.push(TimelineLine::new(
                            "status",
                            format!("{}: {}", index + 1, clip_chars(item, 120)),
                            false,
                        ));
                    }
                }
                self.status = "stash listed".to_string();
                true
            }
            "attach" => {
                let reference = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                if reference.is_empty() {
                    self.timeline.push(TimelineLine::new(
                        "warning",
                        "usage: /attach <path[:range]>",
                        true,
                    ));
                    self.status = "attach invalid".to_string();
                } else {
                    self.input_buffer.clear();
                    self.insert_attachment_reference(reference);
                }
                true
            }
            "themes" | "theme" => {
                let requested = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                if requested.is_empty() {
                    self.timeline.push(TimelineLine::new(
                        "status",
                        format!(
                            "themes: default, light, high-contrast, midnight (current: {})",
                            self.config.theme
                        ),
                        false,
                    ));
                    self.status = "theme updated".to_string();
                } else {
                    self.set_theme(requested);
                }
                true
            }
            "theme-scheme" | "color-scheme" | "scheme" => {
                let requested = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                if requested.is_empty() {
                    self.timeline.push(TimelineLine::new(
                        "status",
                        format!(
                            "color schemes: {} (current: {})",
                            default_color_scheme_names().join(", "),
                            self.config.color_scheme
                        ),
                        false,
                    ));
                    self.status = "color scheme listed".to_string();
                } else if requested == "cycle" || requested == "next" {
                    self.cycle_color_scheme();
                } else {
                    self.set_color_scheme(requested);
                }
                true
            }
            "config" => {
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("tui config: {}", compact_json(&json!(self.config))),
                    false,
                ));
                self.status = "config listed".to_string();
                true
            }
            "keybinds" => {
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("keybinds: {}", compact_json(&json!(self.config.keybinds))),
                    false,
                ));
                self.status = "keybinds listed".to_string();
                true
            }
            "usage" => {
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("usage totals: {}", compact_json(&self.usage_totals)),
                    false,
                ));
                self.status = "usage listed".to_string();
                true
            }
            "warnings" => {
                if self.runtime_warnings.is_empty() {
                    self.timeline
                        .push(TimelineLine::new("status", "warnings: none", false));
                } else {
                    self.timeline.push(TimelineLine::new(
                        "warning",
                        format!("warnings: {} item(s)", self.runtime_warnings.len()),
                        true,
                    ));
                    for warning in self.runtime_warnings.iter().rev().take(10) {
                        self.timeline
                            .push(TimelineLine::new("warning", warning.clone(), false));
                    }
                }
                self.status = "warnings listed".to_string();
                true
            }
            "tool-details" => {
                let requested = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                self.show_tool_details = match requested {
                    "on" | "true" | "1" => true,
                    "off" | "false" | "0" => false,
                    _ => !self.show_tool_details,
                };
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!(
                        "tool details {}",
                        if self.show_tool_details { "on" } else { "off" }
                    ),
                    true,
                ));
                self.status = "tool details toggled".to_string();
                true
            }
            "editor" => {
                self.input_buffer.clear();
                if let Err(error) = self.edit_input_with_external_editor() {
                    self.timeline.push(TimelineLine::new("error", error, true));
                    self.status = "editor failed".to_string();
                }
                true
            }
            _ => false,
        }
    }
}
