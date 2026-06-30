impl FileSessionStore {
    pub fn save_state(&self, session: &Session, run_id: Option<&str>) -> SessionResult<()> {
        let state = SessionStateRecord {
            schema_version: "openagent.session_state.v1".to_string(),
            session_id: session.id.clone(),
            run_id: run_id.map(ToString::to_string),
            workspace: session.directory.to_string_lossy().to_string(),
            status: session.status.clone(),
            updated_at_ms: now_ms(),
            messages: session
                .messages
                .iter()
                .enumerate()
                .map(|(index, message)| stored_message(message, index as u64))
                .collect(),
            todos: session.todos.clone(),
            metadata: session.metadata.clone(),
        };
        write_json(&self.state_path(&session.id), &state)
    }

    pub fn load_session(&self, session_id: &str) -> SessionResult<Session> {
        let state = if let Some(state) = read_json_object(&self.state_path(session_id))? {
            state
        } else {
            self.reconstruct_state_from_transcript(session_id)?
                .ok_or_else(|| format!("Session state not found: {session_id}"))?
        };
        let messages = state
            .get("messages")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        serde_json::from_value::<ChatMessage>(item.clone())
                            .ok()
                            .or_else(|| {
                                serde_json::from_value::<StoredMessage>(item.clone())
                                    .ok()
                                    .map(chat_message_from_stored)
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let todos = state
            .get("todos")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| serde_json::from_value::<TodoItem>(item.clone()).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let metadata = state
            .get("metadata")
            .and_then(Value::as_object)
            .map(|items| items.clone().into_iter().collect())
            .unwrap_or_default();
        Ok(Session {
            id: state
                .get("session_id")
                .and_then(Value::as_str)
                .unwrap_or(session_id)
                .to_string(),
            directory: PathBuf::from(
                state
                    .get("workspace")
                    .and_then(Value::as_str)
                    .unwrap_or("."),
            ),
            status: session_status_from_value(state.get("status")),
            messages,
            todos,
            metadata,
        })
    }

    fn reconstruct_state_from_transcript(
        &self,
        session_id: &str,
    ) -> SessionResult<Option<Map<String, Value>>> {
        let transcript_path = self.transcript_path(session_id);
        if !transcript_path.exists() {
            return Ok(None);
        }
        let session_record =
            read_json_object(&self.session_json_path(session_id))?.unwrap_or_default();
        let messages = read_jsonl(&transcript_path)?;
        Ok(Some(Map::from_iter([
            ("session_id".to_string(), json!(session_id)),
            (
                "workspace".to_string(),
                session_record
                    .get("workspace")
                    .cloned()
                    .unwrap_or_else(|| json!(".")),
            ),
            (
                "status".to_string(),
                session_record
                    .get("status")
                    .cloned()
                    .unwrap_or_else(|| json!("idle")),
            ),
            ("messages".to_string(), Value::Array(messages)),
            ("todos".to_string(), json!([])),
            ("metadata".to_string(), json!({})),
        ])))
    }
}
