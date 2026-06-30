impl RemoteRuntimeClient {
    pub fn start_turn(
        &self,
        session_id: &str,
        prompt: &str,
        mut extra: Value,
    ) -> Result<Value, String> {
        if !extra.is_object() {
            extra = json!({});
        }
        extra["input"] = json!(prompt);
        self.json(
            "POST",
            &format!("/api/sessions/{session_id}/turns"),
            Some(extra),
        )
    }

    pub fn interrupt_turn(&self, turn_id: &str) -> Result<Value, String> {
        self.json("POST", &format!("/api/turns/{turn_id}/interrupt"), None)
    }

    pub fn tasks(&self, session_id: &str) -> Result<Vec<Value>, String> {
        let payload = self.tasks_payload(session_id)?;
        Ok(payload
            .get("tasks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub fn tasks_payload(&self, session_id: &str) -> Result<Value, String> {
        let payload = self.json("GET", &format!("/api/sessions/{session_id}/tasks"), None)?;
        Ok(payload)
    }

    pub fn run_task(&self, session_id: &str, task_id: &str, extra: Value) -> Result<Value, String> {
        self.json(
            "POST",
            &format!("/api/sessions/{session_id}/tasks/{task_id}/run"),
            Some(extra),
        )
    }

    pub fn cancel_task(&self, session_id: &str, task_id: &str) -> Result<Value, String> {
        self.json(
            "POST",
            &format!("/api/sessions/{session_id}/tasks/{task_id}/cancel"),
            None,
        )
    }

    pub fn turn_events(&self, turn_id: &str, last_event_id: u64) -> Result<Vec<Value>, String> {
        let path = if last_event_id == 0 {
            format!("/api/turns/{turn_id}/events")
        } else {
            format!("/api/turns/{turn_id}/events?last_event_id={last_event_id}")
        };
        self.sse_events(&path)
    }

    pub fn global_events(&self, last_event_id: u64) -> Result<Vec<Value>, String> {
        self.sse_events(&format!("/api/events?last_event_id={last_event_id}"))
    }

    pub fn respond_approval(&self, payload: &Value) -> Result<Value, String> {
        let turn_id = string_field(payload, "turn_id");
        let request_id = string_field(payload, "request_id");
        if turn_id.is_empty() || request_id.is_empty() {
            return Err("approval response requires turn_id and request_id".to_string());
        }
        self.json(
            "POST",
            &format!("/api/turns/{turn_id}/approvals/{request_id}"),
            Some(payload.clone()),
        )
    }

    pub fn respond_question(&self, payload: &Value) -> Result<Value, String> {
        let turn_id = string_field(payload, "turn_id");
        let request_id = string_field(payload, "request_id");
        if turn_id.is_empty() || request_id.is_empty() {
            return Err("question response requires turn_id and request_id".to_string());
        }
        self.json(
            "POST",
            &format!("/api/turns/{turn_id}/questions/{request_id}/reply"),
            Some(payload.clone()),
        )
    }

    pub fn next_tui_control(&self) -> Result<Value, String> {
        self.json("GET", "/tui/control/next", None)
    }

    pub fn record_tui_control_response(&self, payload: &Value) -> Result<Value, String> {
        self.json("POST", "/tui/control/response", Some(payload.clone()))
    }
}
