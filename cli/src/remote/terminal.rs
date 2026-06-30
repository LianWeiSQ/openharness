use super::*;

pub(super) struct RemoteTerminalHandler {
    pub(super) url: String,
    pub(super) auth: RemoteAuth,
    pub(super) workspace: PathBuf,
    pub(super) current_session: Option<String>,
    pub(super) last_turn_id: Option<String>,
    pub(super) last_global_event_id: u64,
    pub(super) pending_events: Vec<Value>,
    pub(super) seen_events: BTreeSet<String>,
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
            return Ok(tui_lines("assistant", stable_json_dumps(&payload), false));
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
        if command == "/tasks" {
            let session_id = self.ensure_session()?;
            let payload = remote_tasks_payload(&self.url, &self.auth, &session_id)?;
            return Ok(tui_lines("status", remote_tasks_text(&payload), false));
        }
        if let Some(task_id) = command.strip_prefix("/task ").map(str::trim) {
            if task_id.is_empty() {
                return Ok(tui_lines("warning", "usage: /task <task_session_id>", true));
            }
            self.current_session = Some(task_id.to_string());
            return Ok(tui_lines(
                "status",
                format!("current task session: {task_id}"),
                true,
            ));
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
            "commands: /sessions, /tasks, /task <id>, /resume <id>, /new, /fork, /interrupt [turn_id], /exit",
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
