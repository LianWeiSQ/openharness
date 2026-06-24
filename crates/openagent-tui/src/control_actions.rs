use serde_json::{Value, json};

use crate::{
    ChoicePickerKind, TimelineLine, TuiState,
    attachments::{attachment_reference_from_parts, normalize_attachment_reference_token},
    commands::{BUILTIN_COMMANDS, is_local_state_command},
    config::{
        default_color_scheme_names, default_theme_names, is_valid_color_scheme, string_array,
    },
    control::{control_publish_to_action, control_string_field, normalize_control_action},
    interaction::{answer_vecs, set_action_name},
    picker::{agent_list_lines, model_list_lines},
    util::object_value,
};

impl TuiState {
    pub fn apply_control_request(&mut self, request: &Value) -> Value {
        let path = request
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut action;
        let mut params;
        if !path.is_empty() {
            action = normalize_control_action(path.trim_start_matches("/tui/").trim_matches('/'));
            params = object_value(request.get("body"));
            if action == "publish" {
                (action, params) = control_publish_to_action(&params);
            }
        } else {
            action = request
                .get("action")
                .or_else(|| request.get("type"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            params = object_value(request.get("params"));
        }
        action = normalize_control_action(&action);

        match action.as_str() {
            "prompt.append" => {
                let text = params
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.input_buffer.push_str(text);
                self.status = "prompt updated".to_string();
                json!({"applied": true, "action": action})
            }
            "prompt.submit" => {
                let submitted = self.submit();
                json!({"applied": submitted, "action": action})
            }
            "prompt.clear" => {
                self.input_buffer.clear();
                self.status = "prompt cleared".to_string();
                json!({"applied": true, "action": action})
            }
            "help.open" => {
                self.show_help();
                json!({"applied": true, "action": action})
            }
            "sessions.open" => {
                let query = control_string_field(&params, &["query", "text", "value"]);
                let command = if query.is_empty() {
                    "/sessions".to_string()
                } else {
                    format!("/sessions {query}")
                };
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("queued session picker: {command}"),
                    true,
                ));
                self.status = "session picker queued".to_string();
                json!({"applied": true, "action": action, "command": command})
            }
            "session.select" => {
                let session_id = params
                    .get("sessionID")
                    .or_else(|| params.get("session_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if session_id.is_empty() {
                    self.timeline.push(TimelineLine::new(
                        "error",
                        "control request missing sessionID",
                        true,
                    ));
                    self.status = "control invalid".to_string();
                    return json!({"applied": false, "action": action, "error": "sessionID is required"});
                }
                self.session_id = Some(session_id.to_string());
                self.input_buffer.clear();
                self.timeline.clear();
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("resumed session: {session_id}"),
                    true,
                ));
                self.status = "session resumed".to_string();
                json!({"applied": true, "action": action, "command": format!("/resume {session_id}")})
            }
            "session.rename" => self.session_command_control(&params, &action, "rename"),
            "session.archive" => {
                self.session_literal_command_control(&action, "/archive".to_string())
            }
            "session.unarchive" => {
                self.session_literal_command_control(&action, "/unarchive".to_string())
            }
            "session.delete" => {
                self.session_literal_command_control(&action, "/delete".to_string())
            }
            "session.fork" => self.session_literal_command_control(&action, "/fork".to_string()),
            "session.children" => {
                self.session_literal_command_control(&action, "/children".to_string())
            }
            "session.parent" => {
                self.session_literal_command_control(&action, "/parent".to_string())
            }
            "session.share" => self.session_literal_command_control(&action, "/share".to_string()),
            "session.unshare" => {
                self.session_literal_command_control(&action, "/unshare".to_string())
            }
            "session.compact" => {
                self.session_literal_command_control(&action, "/compact".to_string())
            }
            "session.details" => {
                self.session_literal_command_control(&action, "/details".to_string())
            }
            "session.undo" => self.session_literal_command_control(&action, "/undo".to_string()),
            "session.redo" => self.session_literal_command_control(&action, "/redo".to_string()),
            "toast.show" => {
                let message = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if message.is_empty() {
                    self.status = "control invalid".to_string();
                    return json!({"applied": false, "action": action, "error": "message is required"});
                }
                let title = params
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("toast");
                let variant = params
                    .get("variant")
                    .and_then(Value::as_str)
                    .unwrap_or("status")
                    .to_ascii_lowercase();
                let kind = if matches!(variant.as_str(), "error" | "danger") {
                    "error"
                } else if matches!(variant.as_str(), "warn" | "warning") {
                    "warning"
                } else {
                    "status"
                };
                self.timeline
                    .push(TimelineLine::new(kind, format!("{title}: {message}"), true));
                self.status = title.to_string();
                json!({"applied": true, "action": action})
            }
            "command.execute" => {
                let command = params
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if command.is_empty() {
                    self.status = "control invalid".to_string();
                    return json!({"applied": false, "action": action, "error": "command is required"});
                }
                self.input_buffer = if command.starts_with('/') {
                    command.to_string()
                } else {
                    format!("/{command}")
                };
                if is_local_state_command(&self.input_buffer)
                    || matches!(self.input_buffer.as_str(), "/help" | "/?" | "/")
                {
                    let submitted = self.submit();
                    json!({"applied": submitted, "action": action})
                } else {
                    let command = self.input_buffer.clone();
                    self.input_buffer.clear();
                    self.timeline.push(TimelineLine::new(
                        "status",
                        format!("queued command: {command}"),
                        true,
                    ));
                    self.status = "command queued".to_string();
                    json!({"applied": true, "action": action, "command": command})
                }
            }
            "approval.respond" => self.respond_active_approval_from_params(&params, &action),
            "question.reply" => self.answer_active_question_from_params(&params, &action),
            "question.dismiss" => self.dismiss_active_question_from_params(&params, &action),
            "model.open" => self.open_model_control(&params, &action),
            "model.select" | "model.set" => self.select_model_control(&params, &action),
            "agent.open" => self.open_agent_control(&params, &action),
            "agent.select" | "agent.set" => self.select_agent_control(&params, &action),
            "variant.open" => self.open_variant_control(&params, &action),
            "variant.select" | "variant.set" => {
                self.select_named_session_setting_control(&params, &action, "variant", "variant")
            }
            "thinking.open" => self.open_thinking_control(&params, &action),
            "thinking.select" | "thinking.set" => {
                self.select_named_session_setting_control(&params, &action, "thinking", "level")
            }
            "theme.open" => self.open_theme_control(&params, &action),
            "theme.select" | "theme.set" => self.select_theme_control(&params, &action),
            "theme.scheme.open" => self.open_color_scheme_control(&params, &action),
            "theme.scheme.select" | "theme.scheme.set" => {
                self.select_color_scheme_control(&params, &action)
            }
            "theme.scheme.cycle" => self.cycle_color_scheme_control(&action),
            "palette.open" => self.open_palette_control(&params, &action),
            "palette.execute" => self.execute_palette_control(&params, &action),
            "file.open" => self.open_file_control(&params, &action),
            "file.select" | "file.attach" => self.select_file_control(&params, &action),
            _ => {
                self.timeline.push(TimelineLine::new(
                    "warning",
                    format!(
                        "unknown TUI control: {}",
                        if action.is_empty() { "-" } else { &action }
                    ),
                    true,
                ));
                self.status = "control unknown".to_string();
                json!({"applied": false, "action": action, "unsupported": true})
            }
        }
    }

    fn respond_active_approval_from_params(&mut self, params: &Value, action_name: &str) -> Value {
        let action = params
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let scope = params.get("scope").and_then(Value::as_str);
        let note = params.get("note").and_then(Value::as_str);
        let mut result = self.respond_active_approval(action, scope, note);
        set_action_name(&mut result, action_name);
        result
    }

    fn answer_active_question_from_params(&mut self, params: &Value, action_name: &str) -> Value {
        let answers = params
            .get("answers")
            .cloned()
            .or_else(|| {
                params
                    .get("answer")
                    .and_then(Value::as_str)
                    .map(|answer| json!([[answer]]))
            })
            .and_then(|value| answer_vecs(&value))
            .unwrap_or_default();
        let note = params.get("note").and_then(Value::as_str);
        let mut result = self.answer_active_question(answers, note);
        set_action_name(&mut result, action_name);
        result
    }

    fn dismiss_active_question_from_params(&mut self, params: &Value, action_name: &str) -> Value {
        let note = params.get("note").and_then(Value::as_str);
        let mut result = self.dismiss_active_question(note);
        set_action_name(&mut result, action_name);
        result
    }

    fn open_model_control(&mut self, params: &Value, action_name: &str) -> Value {
        if let Some(models) = params.get("models").and_then(Value::as_array).cloned() {
            self.open_model_picker("", models);
            self.timeline.extend(model_list_lines(params));
            json!({"applied": true, "action": action_name})
        } else {
            self.timeline.push(TimelineLine::new(
                "status",
                "queued model picker: /models",
                true,
            ));
            self.status = "model picker queued".to_string();
            json!({"applied": true, "action": action_name, "command": "/models"})
        }
    }

    fn select_model_control(&mut self, params: &Value, action_name: &str) -> Value {
        let model = control_string_field(params, &["model", "model_id", "modelID", "id"]);
        if model.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "model id is required"});
        }
        let command = format!("/models {model}");
        self.timeline.push(TimelineLine::new(
            "status",
            format!("queued model selection: {model}"),
            true,
        ));
        self.status = "model queued".to_string();
        json!({"applied": true, "action": action_name, "command": command})
    }

    fn open_agent_control(&mut self, params: &Value, action_name: &str) -> Value {
        if let Some(agents) = params.get("agents").and_then(Value::as_array).cloned() {
            self.open_agent_picker("", agents);
            self.timeline.extend(agent_list_lines(params));
            json!({"applied": true, "action": action_name})
        } else {
            self.timeline.push(TimelineLine::new(
                "status",
                "queued agent picker: /agents",
                true,
            ));
            self.status = "agent picker queued".to_string();
            json!({"applied": true, "action": action_name, "command": "/agents"})
        }
    }

    fn select_agent_control(&mut self, params: &Value, action_name: &str) -> Value {
        let agent = control_string_field(params, &["agent", "agent_id", "agentID", "id"]);
        if agent.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "agent id is required"});
        }
        self.queue_session_setting_command(action_name, "agent", &agent)
    }

    fn open_variant_control(&mut self, params: &Value, action_name: &str) -> Value {
        if let Some(variants) = params
            .get("variants")
            .and_then(Value::as_array)
            .map(|items| string_array(items))
            .filter(|items| !items.is_empty())
        {
            self.open_choice_picker(ChoicePickerKind::Variant, "", variants.clone());
            self.timeline.push(TimelineLine::new(
                "status",
                format!("variants: {}", variants.join(", ")),
                true,
            ));
            return json!({"applied": true, "action": action_name, "variants": variants});
        }
        self.timeline.push(TimelineLine::new(
            "status",
            "queued variant picker: /variant",
            true,
        ));
        self.status = "variant picker queued".to_string();
        json!({"applied": true, "action": action_name, "command": "/variant"})
    }

    fn open_thinking_control(&mut self, params: &Value, action_name: &str) -> Value {
        if let Some(levels) = params
            .get("levels")
            .or_else(|| params.get("thinking"))
            .and_then(Value::as_array)
            .map(|items| string_array(items))
            .filter(|items| !items.is_empty())
        {
            self.open_choice_picker(ChoicePickerKind::Thinking, "", levels.clone());
            self.timeline.push(TimelineLine::new(
                "status",
                format!("thinking levels: {}", levels.join(", ")),
                true,
            ));
            return json!({"applied": true, "action": action_name, "levels": levels});
        }
        self.timeline.push(TimelineLine::new(
            "status",
            "queued thinking picker: /thinking",
            true,
        ));
        self.status = "thinking picker queued".to_string();
        json!({"applied": true, "action": action_name, "command": "/thinking"})
    }

    fn select_named_session_setting_control(
        &mut self,
        params: &Value,
        action_name: &str,
        command: &str,
        field: &str,
    ) -> Value {
        let value = control_string_field(params, &[field, "value", "id", "name"]);
        if value.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": format!("{field} is required")});
        }
        self.queue_session_setting_command(action_name, command, &value)
    }

    fn queue_session_setting_command(
        &mut self,
        action_name: &str,
        command: &str,
        value: &str,
    ) -> Value {
        let slash = format!("/{command} {value}");
        self.timeline.push(TimelineLine::new(
            "status",
            format!("queued {command} selection: {value}"),
            true,
        ));
        self.status = format!("{command} queued");
        json!({"applied": true, "action": action_name, "command": slash})
    }

    fn open_theme_control(&mut self, params: &Value, action_name: &str) -> Value {
        let themes = params
            .get("themes")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|items| !items.is_empty())
            .unwrap_or_else(default_theme_names);
        self.open_choice_picker(ChoicePickerKind::Theme, "", themes.clone());
        self.timeline.push(TimelineLine::new(
            "status",
            format!(
                "themes: {} (current: {})",
                themes.join(", "),
                self.config.theme
            ),
            true,
        ));
        json!({"applied": true, "action": action_name, "themes": themes})
    }

    fn select_theme_control(&mut self, params: &Value, action_name: &str) -> Value {
        let theme = control_string_field(params, &["theme", "id", "name"]);
        if theme.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "theme is required"});
        }
        self.set_theme(&theme);
        json!({"applied": true, "action": action_name, "theme": theme})
    }

    fn open_color_scheme_control(&mut self, params: &Value, action_name: &str) -> Value {
        let schemes = params
            .get("schemes")
            .or_else(|| params.get("color_schemes"))
            .and_then(Value::as_array)
            .map(|items| {
                string_array(items)
                    .into_iter()
                    .filter(|scheme| is_valid_color_scheme(scheme))
                    .collect::<Vec<_>>()
            })
            .filter(|items| !items.is_empty())
            .unwrap_or_else(default_color_scheme_names);
        self.open_choice_picker(ChoicePickerKind::ThemeScheme, "", schemes.clone());
        self.timeline.push(TimelineLine::new(
            "status",
            format!(
                "color schemes: {} (current: {})",
                schemes.join(", "),
                self.config.color_scheme
            ),
            true,
        ));
        json!({"applied": true, "action": action_name, "schemes": schemes})
    }

    fn select_color_scheme_control(&mut self, params: &Value, action_name: &str) -> Value {
        let scheme =
            control_string_field(params, &["scheme", "color_scheme", "value", "id", "name"]);
        if scheme.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "scheme is required"});
        }
        let applied = self.set_color_scheme(&scheme);
        json!({"applied": applied, "action": action_name, "scheme": scheme})
    }

    fn cycle_color_scheme_control(&mut self, action_name: &str) -> Value {
        self.cycle_color_scheme();
        json!({"applied": true, "action": action_name, "scheme": self.config.color_scheme})
    }

    fn open_palette_control(&mut self, params: &Value, action_name: &str) -> Value {
        let query = params
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut commands = BUILTIN_COMMANDS
            .iter()
            .filter(|(name, description)| {
                query.is_empty()
                    || name.to_ascii_lowercase().contains(&query)
                    || description.to_ascii_lowercase().contains(&query)
            })
            .take(12)
            .map(|(name, description)| format!("{name} - {description}"))
            .collect::<Vec<_>>();
        if commands.is_empty() {
            commands.push("no commands matched".to_string());
        }
        self.timeline.push(TimelineLine::new(
            "status",
            format!("command palette:\n{}", commands.join("\n")),
            true,
        ));
        self.status = "palette open".to_string();
        json!({"applied": true, "action": action_name, "commands": commands})
    }

    fn execute_palette_control(&mut self, params: &Value, action_name: &str) -> Value {
        let command = control_string_field(params, &["command", "id", "name"]);
        if command.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "command is required"});
        }
        let command = if command.starts_with('/') {
            command
        } else {
            format!("/{command}")
        };
        if is_local_state_command(&command) || matches!(command.as_str(), "/help" | "/?" | "/") {
            self.input_buffer = command;
            let submitted = self.submit();
            json!({"applied": submitted, "action": action_name})
        } else {
            self.timeline.push(TimelineLine::new(
                "status",
                format!("queued palette command: {command}"),
                true,
            ));
            self.status = "palette command queued".to_string();
            json!({"applied": true, "action": action_name, "command": command})
        }
    }

    fn open_file_control(&mut self, params: &Value, action_name: &str) -> Value {
        let query = control_string_field(params, &["query", "text", "value"]);
        let command = if query.is_empty() {
            "/files".to_string()
        } else {
            format!("/files {query}")
        };
        self.timeline.push(TimelineLine::new(
            "status",
            format!("queued file picker: {command}"),
            true,
        ));
        self.status = "file picker queued".to_string();
        json!({"applied": true, "action": action_name, "command": command})
    }

    fn select_file_control(&mut self, params: &Value, action_name: &str) -> Value {
        let path = control_string_field(params, &["path", "file", "id", "value", "name"]);
        if path.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "path is required"});
        }
        let reference = attachment_reference_from_parts(
            &path,
            params
                .get("line")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
            params
                .get("start")
                .or_else(|| params.get("line_start"))
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
            params
                .get("end")
                .or_else(|| params.get("line_end"))
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
        );
        let Some(token) = normalize_attachment_reference_token(&reference) else {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "path cannot be represented as an @ attachment"});
        };
        self.file_picker = None;
        self.insert_attachment_reference(&reference);
        json!({"applied": true, "action": action_name, "reference": token})
    }

    fn session_command_control(&mut self, params: &Value, action_name: &str, verb: &str) -> Value {
        let value = control_string_field(params, &["title", "name", "value", "label"]);
        if value.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "title is required"});
        }
        self.session_literal_command_control(action_name, format!("/{verb} {value}"))
    }

    fn session_literal_command_control(&mut self, action_name: &str, command: String) -> Value {
        self.timeline.push(TimelineLine::new(
            "status",
            format!("queued session command: {command}"),
            true,
        ));
        self.status = "session command queued".to_string();
        json!({"applied": true, "action": action_name, "command": command})
    }
}
