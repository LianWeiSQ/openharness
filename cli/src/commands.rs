use super::*;

pub(super) fn custom_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(command_help());
    }
    match args[0].as_str() {
        "list" | "ls" => custom_command_list(&args[1..]),
        "show" => custom_command_show(&args[1..]),
        "render" => custom_command_render(&args[1..]),
        _ => err_text(2, format!("unknown command action: {}", args[0])),
    }
}

fn custom_command_list(args: &[String]) -> CliRunResult {
    let commands = discover_custom_commands(args);
    let payload =
        json!({"commands": commands.iter().map(|item| item.public(false)).collect::<Vec<_>>()});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!("{} command(s)", commands.len()))
    }
}

fn custom_command_show(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--command-dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "command show requires a name");
    };
    let Some(command) = discover_custom_commands(args)
        .into_iter()
        .find(|item| item.name == *name)
    else {
        return err_text(1, format!("Command not found: {name}"));
    };
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&command.public(true))
    } else {
        ok_text(command.template)
    }
}

fn custom_command_render(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--command-dir", "--format"]);
    let Some(name) = positionals.first() else {
        return err_text(2, "command render requires a name");
    };
    let command_args = positionals.iter().skip(1).cloned().collect::<Vec<_>>();
    let workspace = workspace_from_args(args);
    let Some(command) = discover_custom_commands(args)
        .into_iter()
        .find(|item| item.name == *name)
    else {
        return err_text(1, format!("Command not found: {name}"));
    };
    let rendered = render_custom_template(&command.template, &command_args, &workspace);
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&json!({"command": command.public(false), "prompt": rendered}))
    } else {
        ok_text(rendered)
    }
}

#[derive(Clone, Debug)]
pub(super) struct CustomCommand {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) description: Option<String>,
    pub(super) agent: Option<String>,
    pub(super) model: Option<String>,
    pub(super) template: String,
}

impl CustomCommand {
    fn public(&self, include_template: bool) -> Value {
        let mut object = Map::from_iter([
            ("name".to_string(), json!(self.name)),
            ("path".to_string(), json!(self.path.to_string_lossy())),
            ("scope".to_string(), json!("project")),
            (
                "description".to_string(),
                self.description.clone().map_or(Value::Null, Value::String),
            ),
            (
                "agent".to_string(),
                self.agent.clone().map_or(Value::Null, Value::String),
            ),
            (
                "model".to_string(),
                self.model.clone().map_or(Value::Null, Value::String),
            ),
        ]);
        if include_template {
            object.insert("template".to_string(), json!(self.template));
        }
        Value::Object(object)
    }
}

pub(super) fn discover_custom_commands(args: &[String]) -> Vec<CustomCommand> {
    let workspace = workspace_from_args(args);
    let mut dirs = vec![workspace.join(".openagent/commands")];
    dirs.extend(
        values_for(args, &["--command-dir"])
            .into_iter()
            .map(PathBuf::from),
    );
    let mut commands = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|item| item.to_str()) != Some("md") {
                continue;
            }
            if let Ok(raw) = fs::read_to_string(&path) {
                let name = path
                    .file_stem()
                    .and_then(|item| item.to_str())
                    .unwrap_or_default()
                    .to_string();
                if !name.is_empty() {
                    commands.push(parse_custom_command(name, path, &raw));
                }
            }
        }
    }
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    commands
}

fn parse_custom_command(name: String, path: PathBuf, raw: &str) -> CustomCommand {
    let mut description = None;
    let mut agent = None;
    let mut model = None;
    let mut template = raw.to_string();
    if let Some(rest) = raw.strip_prefix("---")
        && let Some((frontmatter, body)) = rest.split_once("---")
    {
        template = body.trim_start_matches('\n').to_string();
        for line in frontmatter.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let value = value.trim().trim_matches('"').to_string();
                match key.trim() {
                    "description" => description = Some(value),
                    "agent" => agent = Some(value),
                    "model" => model = Some(value),
                    _ => {}
                }
            }
        }
    }
    CustomCommand {
        name,
        path,
        description,
        agent,
        model,
        template,
    }
}

pub(super) fn render_custom_template(template: &str, args: &[String], workspace: &Path) -> String {
    let arguments = args.join(" ");
    let first = args.first().cloned().unwrap_or_default();
    let mut rendered = template
        .replace("$ARGUMENTS", &arguments)
        .replace("$1", &first);
    let mut attachments = Vec::new();
    for word in rendered.split_whitespace() {
        if let Some(path) = word.strip_prefix('@') {
            let clean =
                path.trim_matches(|item: char| matches!(item, ',' | '.' | ';' | ':' | ')' | ']'));
            let target = workspace.join(clean);
            if let Ok(content) = fs::read_to_string(&target) {
                attachments.push(format!(
                    "Attached file: {}\n\n```text\n{}\n```",
                    target.to_string_lossy(),
                    content
                ));
            }
        }
    }
    if !attachments.is_empty() {
        rendered.push_str("\n\n");
        rendered.push_str(&attachments.join("\n\n"));
    }
    rendered
}
