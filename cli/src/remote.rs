use super::*;

#[derive(Clone, Debug, Default)]
pub(super) struct RemoteAuth {
    token: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

pub(super) fn remote_auth_from_args(args: &[String]) -> RemoteAuth {
    RemoteAuth {
        token: value_for(args, &["--server-token"])
            .or_else(|| env::var(DEFAULT_SERVER_TOKEN_ENV).ok())
            .or_else(|| {
                value_for(args, &["--server-token-env"]).and_then(|name| env::var(name).ok())
            }),
        username: value_for(args, &["--username", "-u"]),
        password: value_for(args, &["--password", "-p"]),
    }
}

pub(super) fn remote_select_session(
    server_url: &str,
    token: Option<&str>,
    explicit: Option<String>,
    continue_last: bool,
    workspace: &Path,
) -> Result<String, String> {
    if let Some(session_id) = explicit {
        return Ok(session_id);
    }
    if continue_last {
        let payload = http_json("GET", server_url, "/api/sessions", token, None)?;
        if let Some(session_id) = payload
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("session_id").or_else(|| item.get("id")))
            .and_then(Value::as_str)
        {
            return Ok(session_id.to_string());
        }
    }
    let payload = http_json(
        "POST",
        server_url,
        "/api/sessions",
        token,
        Some(json!({"cwd": workspace.to_string_lossy()})),
    )?;
    payload
        .get("session_id")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "server did not return a session id".to_string())
}

pub(super) fn remote_select_session_with_auth(
    server_url: &str,
    auth: &RemoteAuth,
    explicit: Option<String>,
    continue_last: bool,
    fork: bool,
    workspace: &Path,
) -> Result<String, String> {
    if fork && explicit.is_none() && !continue_last {
        return Err("--fork requires --continue or --session".to_string());
    }
    let base = if let Some(session_id) = explicit {
        Some(session_id)
    } else if continue_last {
        let payload = http_json_with_auth("GET", server_url, "/api/sessions", auth, None)?;
        payload
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("session_id").or_else(|| item.get("id")))
            .and_then(Value::as_str)
            .map(str::to_string)
    } else {
        None
    };
    if !fork && let Some(session_id) = base {
        return Ok(session_id);
    }
    let mut body = json!({"cwd": workspace.to_string_lossy()});
    if let Some(fork_from) = base {
        body["fork_from"] = json!(fork_from);
    }
    let payload = http_json_with_auth("POST", server_url, "/api/sessions", auth, Some(body))?;
    payload
        .get("session_id")
        .or_else(|| payload.get("id"))
        .or_else(|| payload.get("session").and_then(|session| session.get("id")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "server did not return a session id".to_string())
}

pub(super) fn remote_start_turn(
    server_url: &str,
    token: Option<&str>,
    session_id: &str,
    prompt: &str,
) -> Result<Value, String> {
    http_json(
        "POST",
        server_url,
        &format!("/api/sessions/{session_id}/turns"),
        token,
        Some(json!({"input": prompt})),
    )
}

pub(super) fn remote_start_turn_with_auth(
    server_url: &str,
    auth: &RemoteAuth,
    session_id: &str,
    prompt: &str,
) -> Result<Value, String> {
    http_json_with_auth(
        "POST",
        server_url,
        &format!("/api/sessions/{session_id}/turns"),
        auth,
        Some(json!({"input": prompt})),
    )
}

fn remote_turn_events(
    server_url: &str,
    auth: &RemoteAuth,
    turn_id: &str,
    last_event_id: u64,
) -> Result<Vec<Value>, String> {
    let path = if last_event_id > 0 {
        format!("/api/turns/{turn_id}/events?last_event_id={last_event_id}")
    } else {
        format!("/api/turns/{turn_id}/events")
    };
    let raw = http_text_with_auth("GET", server_url, &path, auth, None)?;
    openagent_http_runtime::parse_sse_response_lines(&raw.lines().collect::<Vec<_>>())
}

pub(super) fn remote_events_for_payload(
    server_url: &str,
    auth: &RemoteAuth,
    payload: &Value,
) -> Result<Vec<Value>, String> {
    let fallback = payload
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(turn_id) = remote_turn_id(payload) else {
        return Ok(fallback);
    };
    let mut events = Vec::new();
    let mut seen = BTreeSet::new();
    let mut last_id = 0_u64;
    for event in fallback {
        last_id = last_id.max(app_event_sequence(&event));
        if let Some(key) = app_event_dedupe_key(&event)
            && !seen.insert(key)
        {
            continue;
        }
        events.push(event);
    }
    let deadline = SystemTime::now() + Duration::from_secs(remote_attach_wait_seconds());
    loop {
        match remote_turn_events(server_url, auth, &turn_id, last_id) {
            Ok(next) => {
                let mut advanced = false;
                for event in next {
                    let seq = app_event_sequence(&event);
                    if seq > last_id {
                        last_id = seq;
                    }
                    if let Some(key) = app_event_dedupe_key(&event)
                        && !seen.insert(key)
                    {
                        continue;
                    }
                    advanced = true;
                    events.push(event);
                }
                if events.iter().any(is_terminal_app_event) {
                    return Ok(events);
                }
                if !advanced && SystemTime::now() >= deadline {
                    return Ok(events);
                }
            }
            Err(error) if events.is_empty() => return Err(error),
            Err(_) => return Ok(events),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn remote_attach_wait_seconds() -> u64 {
    env::var("OPENAGENT_ATTACH_WAIT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30)
}

fn app_event_sequence(event: &Value) -> u64 {
    event
        .get("sequence")
        .or_else(|| event.get("global_sequence"))
        .and_then(Value::as_u64)
        .unwrap_or_default()
}

fn app_event_dedupe_key(event: &Value) -> Option<String> {
    Some(format!(
        "{}:{}:{}",
        app_event_sequence(event),
        event.get("method").and_then(Value::as_str).unwrap_or(""),
        python_json_dumps(event.get("params").unwrap_or(&Value::Null))
    ))
}

fn is_terminal_app_event(event: &Value) -> bool {
    matches!(
        event.get("method").and_then(Value::as_str),
        Some("turn/completed" | "turn/failed" | "turn/interrupted")
    ) || matches!(
        event
            .get("params")
            .and_then(|params| params.get("status"))
            .and_then(Value::as_str),
        Some("completed" | "failed" | "interrupted")
    )
}

fn remote_turn_id(payload: &Value) -> Option<String> {
    payload
        .get("turn_id")
        .or_else(|| payload.get("turn").and_then(|turn| turn.get("id")))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn remote_list_sessions(server_url: &str, auth: &RemoteAuth) -> Result<Vec<Value>, String> {
    let payload = http_json_with_auth("GET", server_url, "/api/sessions", auth, None)?;
    Ok(payload
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn remote_sessions_text(sessions: &[Value]) -> String {
    if sessions.is_empty() {
        return "Remote sessions: none\n".to_string();
    }
    let mut text = String::from("Remote sessions:\n");
    for (index, session) in sessions.iter().take(20).enumerate() {
        let id = session
            .get("session_id")
            .or_else(|| session.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let status = session
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let messages = session
            .get("message_count")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let workspace = session
            .get("workspace")
            .and_then(Value::as_str)
            .unwrap_or(".");
        text.push_str(&format!(
            "  {}. {}  status={}  messages={}  workspace={}\n",
            index + 1,
            id,
            status,
            messages,
            workspace
        ));
    }
    if sessions.len() > 20 {
        text.push_str(&format!("  ... {} more\n", sessions.len() - 20));
    }
    text
}

fn http_json(
    method: &str,
    server_url: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Result<Value, String> {
    http_json_with_auth(
        method,
        server_url,
        path,
        &RemoteAuth {
            token: token.map(str::to_string),
            username: None,
            password: None,
        },
        body,
    )
}

fn http_json_with_auth(
    method: &str,
    server_url: &str,
    path: &str,
    auth: &RemoteAuth,
    body: Option<Value>,
) -> Result<Value, String> {
    let raw = http_text_with_auth(method, server_url, path, auth, body)?;
    serde_json::from_str(&raw).map_err(|error| format!("server response was not JSON: {error}"))
}

fn http_text_with_auth(
    method: &str,
    server_url: &str,
    path: &str,
    auth: &RemoteAuth,
    body: Option<Value>,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let url = format!("{}{}", server_url.trim_end_matches('/'), path);
    let mut request = match method {
        "GET" => client.get(url),
        "POST" => client.post(url),
        _ => return Err(format!("unsupported HTTP method: {method}")),
    };
    if let Some(token) = auth.token.as_deref().filter(|value| !value.is_empty()) {
        request = request.bearer_auth(token);
    }
    if let Some(password) = auth.password.as_deref().filter(|value| !value.is_empty()) {
        request = request.basic_auth(
            auth.username.as_deref().unwrap_or("openagent"),
            Some(password),
        );
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .map_err(|error| format!("{method} {path} failed: {error}"))?;
    let status = response.status();
    let raw = response.text().map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "{method} {path} returned HTTP {}: {raw}",
            status.as_u16()
        ));
    }
    Ok(raw)
}

pub(super) fn text_from_app_events(events: &[Value]) -> String {
    let mut text = String::new();
    let mut final_answer = String::new();
    for event in events {
        let method = event
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = event.get("params").unwrap_or(&Value::Null);
        if method == "item/agentMessage/delta" {
            if let Some(delta) = params
                .get("delta")
                .or_else(|| params.get("text"))
                .or_else(|| params.get("event").and_then(|event| event.get("text")))
                .and_then(Value::as_str)
            {
                text.push_str(delta);
            }
        }
        if method == "turn/approval_requested" {
            let tool = params
                .get("approval")
                .and_then(|approval| approval.get("tool_name"))
                .and_then(Value::as_str)
                .unwrap_or("tool");
            text.push_str(&format!("\n[approval required: {tool}]\n"));
        }
        if method == "turn/interrupted" {
            text.push_str("\n[turn interrupted]\n");
        }
        if method == "turn/completed" {
            final_answer = params
                .get("final_answer")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
    }
    if text.is_empty() { final_answer } else { text }
}

pub(super) fn http_runtime_command(args: &[String], web: bool, help: &'static str) -> CliRunResult {
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

pub(super) fn tui_command(args: &[String]) -> CliRunResult {
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

pub(super) fn attach_command(args: &[String]) -> CliRunResult {
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
        "OpenAgent remote attach: {url}\nCommands: /sessions, /resume <id>, /new, /fork, /interrupt [turn_id], /exit\n"
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
                        stdout.push_str(&python_json_dumps(&payload));
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
                        stdout.push_str(&python_json_dumps(&payload));
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

struct RemoteTerminalHandler {
    url: String,
    auth: RemoteAuth,
    workspace: PathBuf,
    current_session: Option<String>,
    last_turn_id: Option<String>,
    last_global_event_id: u64,
    pending_events: Vec<Value>,
    seen_events: BTreeSet<String>,
}

impl RemoteTerminalHandler {
    fn ensure_session(&mut self) -> Result<String, String> {
        if let Some(session_id) = self.current_session.clone() {
            return Ok(session_id);
        }
        let session_id = remote_select_session_with_auth(
            &self.url,
            &self.auth,
            None,
            false,
            false,
            &self.workspace,
        )?;
        self.current_session = Some(session_id.clone());
        Ok(session_id)
    }

    fn remember_payload_events(&mut self, payload: &Value) {
        let events = payload
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let events = self.filter_new_events(events);
        self.pending_events.extend(events);
    }

    fn filter_new_events(&mut self, events: Vec<Value>) -> Vec<Value> {
        let mut output = Vec::new();
        for event in events {
            let sequence = app_event_sequence(&event);
            if sequence > self.last_global_event_id {
                self.last_global_event_id = sequence;
            }
            if let Some(key) = app_event_dedupe_key(&event)
                && !self.seen_events.insert(key)
            {
                continue;
            }
            output.push(event);
        }
        output
    }
}

impl openagent_tui::TerminalEventHandler for RemoteTerminalHandler {
    fn initial_lines(&mut self) -> Vec<openagent_tui::TimelineLine> {
        let mut lines = tui_lines("status", format!("connected to {}", self.url), true);
        match remote_list_sessions(&self.url, &self.auth) {
            Ok(sessions) => {
                lines.extend(tui_lines("status", remote_sessions_text(&sessions), false))
            }
            Err(error) => lines.extend(tui_lines("warning", error, true)),
        }
        if let Some(session_id) = self.current_session.as_deref() {
            lines.extend(tui_lines(
                "status",
                format!("current session: {session_id}"),
                true,
            ));
        }
        lines
    }

    fn poll_app_events(&mut self) -> Result<Vec<Value>, String> {
        let raw = http_text_with_auth(
            "GET",
            &self.url,
            &format!("/api/events?last_event_id={}", self.last_global_event_id),
            &self.auth,
            None,
        )?;
        let events =
            openagent_http_runtime::parse_sse_response_lines(&raw.lines().collect::<Vec<_>>())?;
        Ok(self.filter_new_events(events))
    }

    fn poll_control_request(&mut self) -> Result<Option<Value>, String> {
        let payload = http_json_with_auth("GET", &self.url, "/tui/control/next", &self.auth, None)?;
        let path = payload
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if path.is_empty() {
            return Ok(None);
        }
        Ok(Some(payload))
    }

    fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
        http_json_with_auth(
            "POST",
            &self.url,
            "/tui/control/response",
            &self.auth,
            Some(payload.clone()),
        )
        .map(|_| ())
    }

    fn drain_app_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.pending_events)
    }

    fn handle_submit(&mut self, prompt: &str) -> Result<Vec<openagent_tui::TimelineLine>, String> {
        let session_id = self.ensure_session()?;
        let payload = remote_start_turn_with_auth(&self.url, &self.auth, &session_id, prompt)?;
        self.last_turn_id = remote_turn_id(&payload).or_else(|| self.last_turn_id.clone());
        let events = remote_events_for_payload(&self.url, &self.auth, &payload)?;
        if events.is_empty() {
            return Ok(tui_lines("assistant", python_json_dumps(&payload), false));
        }
        let events = self.filter_new_events(events);
        self.pending_events.extend(events);
        Ok(Vec::new())
    }

    fn handle_command(
        &mut self,
        command: &str,
    ) -> Result<Vec<openagent_tui::TimelineLine>, String> {
        if command == "/sessions" {
            let sessions = remote_list_sessions(&self.url, &self.auth)?;
            return Ok(tui_lines("status", remote_sessions_text(&sessions), false));
        }
        if let Some(session_id) = command.strip_prefix("/resume ").map(str::trim) {
            if session_id.is_empty() {
                return Ok(tui_lines("warning", "usage: /resume <session_id>", true));
            }
            self.current_session = Some(session_id.to_string());
            return Ok(tui_lines(
                "status",
                format!("current session: {session_id}"),
                true,
            ));
        }
        if command == "/new" {
            let session_id = remote_select_session_with_auth(
                &self.url,
                &self.auth,
                None,
                false,
                false,
                &self.workspace,
            )?;
            self.current_session = Some(session_id.clone());
            return Ok(tui_lines(
                "status",
                format!("created session: {session_id}"),
                true,
            ));
        }
        if command == "/fork" {
            let Some(base) = self.current_session.clone() else {
                return Ok(tui_lines(
                    "warning",
                    "no current session to fork; use /new or /resume <session_id>",
                    true,
                ));
            };
            let session_id = remote_select_session_with_auth(
                &self.url,
                &self.auth,
                Some(base),
                false,
                true,
                &self.workspace,
            )?;
            self.current_session = Some(session_id.clone());
            return Ok(tui_lines(
                "status",
                format!("forked session: {session_id}"),
                true,
            ));
        }
        if command == "/interrupt" || command.starts_with("/interrupt ") {
            let turn_id = command
                .strip_prefix("/interrupt ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| self.last_turn_id.clone());
            let Some(turn_id) = turn_id else {
                return Ok(tui_lines("warning", "no turn to interrupt", true));
            };
            let payload = http_json_with_auth(
                "POST",
                &self.url,
                &format!("/api/turns/{turn_id}/interrupt"),
                &self.auth,
                None,
            )?;
            self.remember_payload_events(&payload);
            return Ok(Vec::new());
        }
        Ok(tui_lines(
            "status",
            "commands: /sessions, /resume <id>, /new, /fork, /interrupt [turn_id], /exit",
            false,
        ))
    }

    fn handle_approval_response(
        &mut self,
        payload: &Value,
    ) -> Result<Vec<openagent_tui::TimelineLine>, String> {
        let turn_id = payload
            .get("turn_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "approval response missing turn_id".to_string())?;
        let request_id = payload
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "approval response missing request_id".to_string())?;
        let response = http_json_with_auth(
            "POST",
            &self.url,
            &format!("/api/turns/{turn_id}/approvals/{request_id}"),
            &self.auth,
            Some(payload.clone()),
        )?;
        self.remember_payload_events(&response);
        Ok(Vec::new())
    }

    fn handle_question_response(
        &mut self,
        payload: &Value,
    ) -> Result<Vec<openagent_tui::TimelineLine>, String> {
        let turn_id = payload
            .get("turn_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "question response missing turn_id".to_string())?;
        let request_id = payload
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "question response missing request_id".to_string())?;
        let response = http_json_with_auth(
            "POST",
            &self.url,
            &format!("/api/turns/{turn_id}/questions/{request_id}/reply"),
            &self.auth,
            Some(payload.clone()),
        )?;
        self.remember_payload_events(&response);
        Ok(Vec::new())
    }
}

fn tui_lines(
    kind: &str,
    text: impl Into<String>,
    important: bool,
) -> Vec<openagent_tui::TimelineLine> {
    let text = text.into();
    if text.trim().is_empty() {
        return Vec::new();
    }
    text.lines()
        .map(|line| openagent_tui::TimelineLine::new(kind, line.to_string(), important))
        .collect()
}
