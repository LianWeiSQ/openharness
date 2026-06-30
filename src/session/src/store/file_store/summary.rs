impl FileSessionStore {
    pub fn write_run_summary(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> SessionResult<RunSummaryRecord> {
        let events = read_jsonl(&self.events_path(session_id, run_id))?;
        let parts = read_jsonl(&self.parts_path(session_id, run_id))?;
        let mut summary = RunSummaryRecord {
            schema_version: "openagent.run_summary.v1".to_string(),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            event_count: events.len() as u64,
            part_count: parts.len() as u64,
            part_type_counts: count_by_key(&parts, "type"),
            message_count: count_events(&events, "message.appended"),
            step_count: count_events(&events, "step.finished"),
            tool_call_count: events
                .iter()
                .filter(|event| {
                    matches!(
                        event.get("event").and_then(Value::as_str),
                        Some("tool.call.finished" | "tool.call.failed")
                    )
                })
                .count() as u64,
            runtime_warning_count: count_events(&events, "runtime.warning"),
            patch_count: count_events(&events, "patch.detected"),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            status: "running".to_string(),
        };
        for event in &events {
            let attrs = event
                .get("attributes")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            match event.get("event").and_then(Value::as_str) {
                Some("model.usage") => {
                    summary.total_input_tokens += attrs
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    summary.total_output_tokens += attrs
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    summary.total_cost += attrs
                        .get("cost")
                        .and_then(Value::as_f64)
                        .unwrap_or_default();
                }
                Some("run.finished") => {
                    summary.status = attrs
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("completed")
                        .to_string();
                }
                Some("run.failed") => {
                    summary.status = attrs
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("failed")
                        .to_string();
                }
                _ => {}
            }
        }
        write_json(&self.summary_path(session_id, run_id), &summary)?;
        Ok(summary)
    }

    #[must_use]
    pub fn metadata(&self, session_id: &str, run_id: &str) -> SessionStoreMetadata {
        SessionStoreMetadata {
            enabled: true,
            store_type: "file".to_string(),
            root_dir: self.root.to_string_lossy().to_string(),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            session_dir: self.session_dir(session_id).to_string_lossy().to_string(),
            ledger_path: self
                .events_path(session_id, run_id)
                .to_string_lossy()
                .to_string(),
            transcript_path: self
                .transcript_path(session_id)
                .to_string_lossy()
                .to_string(),
            state_path: self.state_path(session_id).to_string_lossy().to_string(),
            run_dir: self
                .run_dir(session_id, run_id)
                .to_string_lossy()
                .to_string(),
            parts_path: self
                .parts_path(session_id, run_id)
                .to_string_lossy()
                .to_string(),
        }
    }
}
