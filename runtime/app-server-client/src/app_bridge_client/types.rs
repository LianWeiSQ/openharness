#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn protocol_crate_name() -> &'static str {
    openagent_protocol::crate_name()
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RemoteEventKey {
    Global(u64),
    Turn {
        turn_id: String,
        sequence: u64,
        method: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteTurnRecord {
    pub id: String,
    pub session_id: String,
    pub status: String,
    pub final_answer: String,
    pub error: Option<String>,
    pub trace: Option<Value>,
    pub events: Vec<AppEvent>,
    seen_event_keys: BTreeSet<RemoteEventKey>,
}

impl RemoteTurnRecord {
    #[must_use]
    pub fn new(id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            session_id: session_id.into(),
            status: "queued".to_string(),
            final_answer: String::new(),
            error: None,
            trace: None,
            events: Vec::new(),
            seen_event_keys: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn from_payload(payload: &Value, session_id: &str) -> Self {
        Self {
            id: string_field(payload, "id"),
            session_id: string_field(payload, "session_id")
                .if_empty_then(|| session_id.to_string()),
            status: string_field(payload, "status").if_empty_then(|| "queued".to_string()),
            final_answer: string_field(payload, "final_answer"),
            error: optional_string_field(payload, "error"),
            trace: payload
                .get("trace")
                .filter(|value| value.is_object())
                .cloned(),
            events: Vec::new(),
            seen_event_keys: BTreeSet::new(),
        }
    }

    pub fn append_event(&mut self, event: AppEvent) -> bool {
        let key = remote_event_key(&event, &self.id);
        if self.seen_event_keys.contains(&key) {
            return false;
        }
        self.seen_event_keys.insert(key);
        self.apply_event(&event);
        self.events.push(event);
        true
    }

    pub fn mark_failed(&mut self, error: impl Into<String>) {
        self.status = "failed".to_string();
        self.error = Some(error.into());
    }

    fn apply_event(&mut self, event: &AppEvent) {
        match event.method.as_str() {
            "turn/approval_requested" => {
                self.status = string_field(&event.params, "status")
                    .if_empty_then(|| "waiting_approval".to_string());
            }
            "turn/approval_resolved" | "turn/started" => {
                self.status =
                    string_field(&event.params, "status").if_empty_then(|| "running".to_string());
            }
            method if TERMINAL_METHODS.contains(&method) => {
                let default_status = match method {
                    "turn/completed" => "completed",
                    "turn/interrupted" => "interrupted",
                    _ => "failed",
                };
                self.status = string_field(&event.params, "status")
                    .if_empty_then(|| default_status.to_string());
                let final_answer = string_field(&event.params, "final_answer");
                if !final_answer.is_empty() {
                    self.final_answer = final_answer;
                }
                if let Some(error) = optional_string_field(&event.params, "error") {
                    self.error = Some(error);
                }
                if let Some(trace) = event.params.get("trace").filter(|value| value.is_object()) {
                    self.trace = Some(trace.clone());
                }
            }
            _ => {}
        }
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATUSES.contains(&self.status.as_str())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoteAuth {
    pub token: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl RemoteAuth {
    #[must_use]
    pub fn bearer(token: impl Into<String>) -> Self {
        Self {
            token: Some(token.into()),
            username: None,
            password: None,
        }
    }

    #[must_use]
    pub fn basic(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            token: None,
            username: Some(username.into()),
            password: Some(password.into()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteRuntimeClient {
    server_url: String,
    auth: RemoteAuth,
    timeout: Duration,
}
