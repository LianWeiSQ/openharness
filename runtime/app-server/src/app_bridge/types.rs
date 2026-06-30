#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn core_crate_name() -> &'static str {
    openagent_core::crate_name()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AppEvent {
    pub sequence: u64,
    pub method: String,
    pub params: Value,
    pub created_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_sequence: Option<u64>,
}

impl AppEvent {
    #[must_use]
    pub fn new(
        sequence: u64,
        method: impl Into<String>,
        params: Value,
        created_at_ms: u64,
    ) -> Self {
        Self {
            sequence,
            method: method.into(),
            params: json_safe(params),
            created_at_ms,
            global_sequence: None,
        }
    }

    #[must_use]
    pub fn with_global_sequence(mut self, global_sequence: u64) -> Self {
        self.global_sequence = Some(global_sequence);
        self
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TuiControlRequest {
    pub path: String,
    pub body: Value,
}

impl TuiControlRequest {
    #[must_use]
    pub fn new(path: impl Into<String>, body: Value) -> Self {
        Self {
            path: path.into(),
            body: json_safe(body),
        }
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SseReplayEvent {
    pub id: String,
    pub event: String,
    pub data: Value,
}

impl SseReplayEvent {
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

#[derive(Clone, Debug, Default)]
pub struct TuiControlQueue {
    requests: VecDeque<TuiControlRequest>,
    responses: VecDeque<Value>,
}

impl TuiControlQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(
        &mut self,
        path: impl Into<String>,
        body: Value,
    ) -> Result<TuiControlRequest, String> {
        if self.requests.len() >= MAX_TUI_CONTROL_QUEUE {
            return Err("TUI control queue is full".to_string());
        }
        let request = TuiControlRequest::new(path, body);
        self.requests.push_back(request.clone());
        Ok(request)
    }

    pub fn pop_next_request(&mut self) -> Option<TuiControlRequest> {
        self.requests.pop_front()
    }

    pub fn record_response(&mut self, payload: Value) -> Value {
        self.responses.push_back(payload.clone());
        payload
    }

    pub fn next_response(&mut self) -> Option<Value> {
        self.responses.pop_front()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TurnRecord {
    pub id: String,
    pub session_id: String,
    pub input: String,
    pub created_at_ms: u64,
    pub status: String,
    pub final_answer: String,
    pub error: Option<String>,
    pub trace: Option<Value>,
    pub interrupt_requested: bool,
    pub events: Vec<AppEvent>,
    pub pending_approval_count: u64,
}

impl TurnRecord {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        session_id: impl Into<String>,
        input: impl Into<String>,
        created_at_ms: u64,
    ) -> Self {
        Self {
            id: id.into(),
            session_id: session_id.into(),
            input: input.into(),
            created_at_ms,
            status: "queued".to_string(),
            final_answer: String::new(),
            error: None,
            trace: None,
            interrupt_requested: false,
            events: Vec::new(),
            pending_approval_count: 0,
        }
    }

    #[must_use]
    pub fn request_interrupt(&mut self, created_at_ms: u64) -> Option<AppEvent> {
        if matches!(self.status.as_str(), "completed" | "failed" | "interrupted") {
            return None;
        }
        if self.interrupt_requested {
            return None;
        }
        self.interrupt_requested = true;
        self.status = "interrupting".to_string();
        let event = lifecycle_event(
            self.events.len() as u64 + 1,
            "turn/interrupt_requested",
            &self.session_id,
            Some(&self.id),
            json!({"status": self.status}),
            created_at_ms,
        );
        self.events.push(event.clone());
        Some(event)
    }

    #[must_use]
    pub fn to_runtime_value(&self) -> Value {
        json!({
            "id": self.id,
            "session_id": self.session_id,
            "status": self.status,
            "created_at_ms": self.created_at_ms,
            "final_answer": self.final_answer,
            "error": self.error,
            "trace": self.trace,
            "event_count": self.events.len(),
            "interrupt_requested": self.interrupt_requested,
            "pending_approval_count": self.pending_approval_count,
        })
    }
}
