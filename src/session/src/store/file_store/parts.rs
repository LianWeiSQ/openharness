impl FileSessionStore {
    pub fn append_part(
        &self,
        session_id: &str,
        run_id: &str,
        part_type: &str,
        options: SessionPartOptions,
    ) -> SessionResult<SessionPartRecord> {
        let parts_path = self.parts_path(session_id, run_id);
        let message_id = options.message_id;
        let content = options.content;
        let attributes = options.attributes;
        let status = options.status;
        let step_index = options.step_index;
        let timestamp_ms = options.timestamp_ms.unwrap_or_else(now_ms);
        let payload = SessionPartRecord {
            schema_version: "openagent.session_part.v1".to_string(),
            part_id: options.part_id.unwrap_or_else(|| new_id("part")),
            seq: next_seq(&parts_path)?,
            part_type: part_type.to_string(),
            timestamp_ms,
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            step_index,
            status: normalize_part_status(&status),
            attributes,
        };
        append_jsonl(&parts_path, &payload)?;
        if let Some(message_id) = message_id.as_deref() {
            let part = MessagePart {
                id: payload.part_id.clone(),
                message_id: message_id.to_string(),
                session_id: session_id.to_string(),
                seq: self.next_message_part_seq(session_id, message_id)?,
                kind: MessagePartKind::from_type(part_type),
                status: message_status_from_str(&payload.status),
                content: content.unwrap_or_else(|| {
                    Value::Object(payload.attributes.clone().into_iter().collect())
                }),
                attributes: payload.attributes.clone(),
                timestamp_ms: payload.timestamp_ms,
                run_id: Some(run_id.to_string()),
                step_index: payload.step_index,
            };
            self.append_message_part_v2(session_id, part)?;
        }
        self.write_run_summary(session_id, run_id)?;
        Ok(payload)
    }

    pub fn load_parts(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> SessionResult<Vec<SessionPartRecord>> {
        read_jsonl(&self.parts_path(session_id, run_id))?
            .into_iter()
            .map(|value| serde_json::from_value::<SessionPartRecord>(value).map_err(Into::into))
            .collect()
    }
}
