use super::*;

pub(crate) fn http_runtime_command(args: &[String], web: bool, help: &'static str) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(help);
    }
    let mut runtime_args = args.to_vec();
    if !web && !has_flag(args, &["--headless"]) {
        runtime_args.push("--headless".to_string());
    }
    let result = openagent_http_runtime::run_cli(&runtime_args);
    CliRunResult {
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
    }
}

pub(crate) fn tui_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(tui_help());
    }
    if value_for(args, &["--attach"]).is_some() {
        return attach_command(args);
    }
    if let Some(prompt) = value_for(args, &["--prompt"]) {
        let mut run_args = vec!["--skip-doctor".to_string()];
        run_args.extend(args.iter().filter(|arg| *arg != "--prompt").cloned());
        run_args.push(prompt);
        return run_prompt_command(&run_args);
    }
    if !io::stdin().is_terminal() {
        return ok_text("openagent-tui ready; pass --prompt or use an interactive terminal");
    }
    interactive_local_loop(args)
}

pub(crate) fn attach_command(args: &[String]) -> CliRunResult {
    if args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(attach_help());
    }
    let positionals = positional_args(
        args,
        &[
            "--workspace",
            "--dir",
            "--session",
            "-s",
            "--server-token",
            "--server-token-env",
            "--username",
            "-u",
            "--password",
            "-p",
            "--attach",
            "--format",
        ],
    );
    let Some(url) = value_for(args, &["--attach"]).or_else(|| positionals.first().cloned()) else {
        return err_text(2, "openagent attach requires a server URL");
    };
    let auth = remote_auth_from_args(args);
    let mut health = json!({});
    if !has_flag(args, &["--skip-health-check"])
        && let Err(error) =
            http_json_with_auth("GET", &url, "/api/health", &auth, None).map(|payload| {
                health = payload;
            })
    {
        return err_text(1, error);
    }
    if !io::stdin().is_terminal() {
        let sessions = remote_list_sessions(&url, &auth).unwrap_or_default();
        if value_for(args, &["--format"]).as_deref() == Some("json") {
            return CliRunResult::ok_json(&json!({
                "attached": true,
                "server_url": url,
                "health": health,
                "sessions": sessions,
            }));
        }
        let mut output = format!("attached to {url}\n");
        output.push_str(&remote_sessions_text(&sessions));
        return ok_text(output);
    }
    interactive_remote_loop(args, &url, &auth)
}

fn interactive_local_loop(args: &[String]) -> CliRunResult {
    let mut stdout = String::new();
    stdout.push_str("OpenAgent TUI direct mode. Type /exit to quit.\n");
    let mut line = String::new();
    loop {
        line.clear();
        if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let prompt = line.trim();
        if matches!(prompt, "/exit" | "/quit") {
            break;
        }
        if prompt.is_empty() {
            continue;
        }
        let mut run_args = args.to_vec();
        run_args.push("--skip-doctor".to_string());
        run_args.push(prompt.to_string());
        let result = run_prompt_command(&run_args);
        stdout.push_str(&result.stdout);
        if result.exit_code != 0 {
            return CliRunResult {
                exit_code: result.exit_code,
                stdout,
                stderr: result.stderr,
            };
        }
    }
    ok_text(stdout)
}

fn interactive_remote_loop(args: &[String], url: &str, auth: &RemoteAuth) -> CliRunResult {
    let workspace = workspace_from_args(args);
    let mut current_session = match remote_select_session_with_auth(
        url,
        auth,
        value_for(args, &["--session", "-s"]),
        has_flag(args, &["--continue", "-c"]),
        has_flag(args, &["--fork"]),
        &workspace,
    ) {
        Ok(session_id) => Some(session_id),
        Err(error) if has_flag(args, &["--continue", "-c"]) || has_flag(args, &["--fork"]) => {
            return err_text(1, error);
        }
        Err(_) => None,
    };
    if io::stdout().is_terminal() {
        let handler = RemoteTerminalHandler {
            url: url.to_string(),
            auth: auth.clone(),
            workspace,
            current_session,
            last_turn_id: None,
            last_global_event_id: 0,
            pending_events: Vec::new(),
            seen_events: BTreeSet::new(),
        };
        return match openagent_tui::run_terminal_ui(
            openagent_tui::TerminalUiOptions {
                title: format!("OpenAgent remote attach: {url}"),
                status: "connected".to_string(),
            },
            handler,
        ) {
            Ok(()) => ok_text(""),
            Err(error) => err_text(1, error),
        };
    }
    let mut last_turn_id: Option<String> = None;
    let mut stdout = format!(
        "OpenAgent remote attach: {url}\nCommands: /sessions, /tasks, /task <id>, /resume <id>, /new, /fork, /interrupt [turn_id], /exit\n"
    );
    if let Ok(sessions) = remote_list_sessions(url, auth) {
        stdout.push_str(&remote_sessions_text(&sessions));
    }
    if let Some(session_id) = current_session.as_deref() {
        stdout.push_str(&format!("Current session: {session_id}\n"));
    }
    let mut line = String::new();
    loop {
        line.clear();
        if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let prompt = line.trim();
        if matches!(prompt, "/exit" | "/quit") {
            break;
        }
        if prompt.is_empty() {
            continue;
        }
        if prompt == "/sessions" {
            match remote_list_sessions(url, auth) {
                Ok(sessions) => stdout.push_str(&remote_sessions_text(&sessions)),
                Err(error) => return err_text(1, error),
            }
            continue;
        }
        if prompt == "/tasks" {
            let Some(session_id) = current_session.as_deref() else {
                stdout.push_str("No current session. Use /new or /resume <session_id>.\n");
                continue;
            };
            match remote_tasks_payload(url, auth, session_id) {
                Ok(payload) => stdout.push_str(&remote_tasks_text(&payload)),
                Err(error) => return err_text(1, error),
            }
            continue;
        }
        if let Some(task_id) = prompt.strip_prefix("/task ").map(str::trim) {
            if task_id.is_empty() {
                stdout.push_str("Usage: /task <task_session_id>\n");
            } else {
                current_session = Some(task_id.to_string());
                stdout.push_str(&format!("Current task session: {task_id}\n"));
            }
            continue;
        }
        if let Some(session_id) = prompt.strip_prefix("/resume ").map(str::trim) {
            if session_id.is_empty() {
                stdout.push_str("Usage: /resume <session_id>\n");
            } else {
                current_session = Some(session_id.to_string());
                stdout.push_str(&format!("Current session: {session_id}\n"));
            }
            continue;
        }
        if prompt == "/new" {
            match remote_select_session_with_auth(url, auth, None, false, false, &workspace) {
                Ok(session_id) => {
                    stdout.push_str(&format!("Created session: {session_id}\n"));
                    current_session = Some(session_id);
                }
                Err(error) => return err_text(1, error),
            }
            continue;
        }
        if prompt == "/fork" {
            let Some(base) = current_session.clone() else {
                stdout.push_str("No current session to fork. Use /new or /resume <session_id>.\n");
                continue;
            };
            match remote_select_session_with_auth(url, auth, Some(base), false, true, &workspace) {
                Ok(session_id) => {
                    stdout.push_str(&format!("Forked session: {session_id}\n"));
                    current_session = Some(session_id);
                }
                Err(error) => return err_text(1, error),
            }
            continue;
        }
        if prompt == "/interrupt" || prompt.starts_with("/interrupt ") {
            let turn_id = prompt
                .strip_prefix("/interrupt ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| last_turn_id.clone());
            let Some(turn_id) = turn_id else {
                stdout.push_str("No turn to interrupt. Pass /interrupt <turn_id>.\n");
                continue;
            };
            match http_json_with_auth(
                "POST",
                url,
                &format!("/api/turns/{turn_id}/interrupt"),
                auth,
                None,
            ) {
                Ok(payload) => {
                    if let Some(events) = payload.get("events").and_then(Value::as_array) {
                        stdout.push_str(&text_from_app_events(events));
                        stdout.push('\n');
                    } else {
                        stdout.push_str(&stable_json_dumps(&payload));
                        stdout.push('\n');
                    }
                }
                Err(error) => return err_text(1, error),
            }
            continue;
        }
        let session_id = match current_session.clone() {
            Some(session_id) => session_id,
            None => {
                match remote_select_session_with_auth(url, auth, None, false, false, &workspace) {
                    Ok(session_id) => {
                        stdout.push_str(&format!("Created session: {session_id}\n"));
                        current_session = Some(session_id.clone());
                        session_id
                    }
                    Err(error) => return err_text(1, error),
                }
            }
        };
        match remote_start_turn_with_auth(url, auth, &session_id, prompt) {
            Ok(payload) => {
                last_turn_id = remote_turn_id(&payload).or(last_turn_id);
                match remote_events_for_payload(url, auth, &payload) {
                    Ok(events) if !events.is_empty() => {
                        stdout.push_str(&text_from_app_events(&events));
                        stdout.push('\n');
                    }
                    Ok(_) => {
                        stdout.push_str(&stable_json_dumps(&payload));
                        stdout.push('\n');
                    }
                    Err(error) => return err_text(1, error),
                }
            }
            Err(error) => return err_text(1, error),
        }
    }
    ok_text(stdout)
}
