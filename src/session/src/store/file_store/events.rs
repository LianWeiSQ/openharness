impl FileSessionStore {
    pub fn record_event(
        &self,
        session_id: &str,
        run_id: &str,
        event: &str,
        options: SessionEventOptions,
    ) -> SessionResult<SessionEventRecord> {
        let event_path = self.events_path(session_id, run_id);
        let payload = SessionEventRecord {
            schema_version: "openagent.session_event.v1".to_string(),
            seq: next_seq(&event_path)?,
            event: event.to_string(),
            timestamp_ms: options.timestamp_ms.unwrap_or_else(now_ms),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            kind: options.kind,
            status: if options.status == "error" {
                "error"
            } else {
                "ok"
            }
            .to_string(),
            duration_ms: options.duration_ms,
            attributes: options.attributes,
        };
        append_jsonl(&event_path, &payload)?;
        self.write_run_summary(session_id, run_id)?;
        Ok(payload)
    }
}
