impl RemoteRuntimeClient {
    pub fn list_sessions(&self) -> Result<Vec<Value>, String> {
        let payload = self.json("GET", "/api/sessions", None)?;
        Ok(payload
            .get("sessions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub fn get_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("GET", &format!("/api/sessions/{session_id}"), None)
    }

    pub fn session_messages(
        &self,
        session_id: &str,
        limit: Option<usize>,
    ) -> Result<Value, String> {
        let path = limit.map_or_else(
            || format!("/api/sessions/{session_id}/messages"),
            |limit| format!("/api/sessions/{session_id}/messages?limit={limit}"),
        );
        self.json("GET", &path, None)
    }

    pub fn search_sessions(&self, query: &str) -> Result<Vec<Value>, String> {
        let path = if query.trim().is_empty() {
            "/api/sessions".to_string()
        } else {
            format!("/api/sessions?query={}", quote_path(query.trim()))
        };
        let payload = self.json("GET", &path, None)?;
        Ok(payload
            .get("sessions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub fn create_session(
        &self,
        workspace: &Path,
        fork_from: Option<&str>,
    ) -> Result<String, String> {
        let mut body = json!({"cwd": workspace.to_string_lossy()});
        if let Some(fork_from) = fork_from.filter(|value| !value.is_empty()) {
            body["fork_from"] = json!(fork_from);
        }
        let payload = self.json("POST", "/api/sessions", Some(body))?;
        session_id_from_payload(&payload)
            .ok_or_else(|| "server did not return a session id".to_string())
    }

    pub fn select_session(
        &self,
        explicit: Option<String>,
        continue_last: bool,
        fork: bool,
        workspace: &Path,
    ) -> Result<String, String> {
        if fork && explicit.is_none() && !continue_last {
            return Err("fork requires an explicit session or continue_last".to_string());
        }
        let base = if let Some(session_id) = explicit {
            Some(session_id)
        } else if continue_last {
            self.list_sessions()?.first().and_then(session_id_from_payload)
        } else {
            None
        };
        if !fork && let Some(session_id) = base {
            return Ok(session_id);
        }
        self.create_session(workspace, base.as_deref())
    }

    pub fn update_session(&self, session_id: &str, body: Value) -> Result<Value, String> {
        self.json("PATCH", &format!("/api/sessions/{session_id}"), Some(body))
    }

    pub fn delete_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("DELETE", &format!("/api/sessions/{session_id}"), None)
    }

    pub fn children(&self, session_id: &str) -> Result<Vec<Value>, String> {
        let payload = self.json("GET", &format!("/api/sessions/{session_id}/children"), None)?;
        Ok(payload
            .get("children")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub fn share_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("POST", &format!("/api/sessions/{session_id}/share"), None)
    }

    pub fn unshare_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("DELETE", &format!("/api/sessions/{session_id}/share"), None)
    }

    pub fn compact_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("POST", &format!("/api/sessions/{session_id}/compact"), None)
    }

    pub fn session_diff(&self, session_id: &str) -> Result<Value, String> {
        self.json("GET", &format!("/api/sessions/{session_id}/diff"), None)
    }

    pub fn undo_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("POST", &format!("/api/sessions/{session_id}/undo"), None)
    }

    pub fn redo_session(&self, session_id: &str) -> Result<Value, String> {
        self.json("POST", &format!("/api/sessions/{session_id}/redo"), None)
    }
}
