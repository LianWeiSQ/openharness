impl FileSessionStore {
    pub fn start_run(
        &self,
        session: &mut Session,
        options: StartRunOptions,
    ) -> SessionResult<SessionStoreMetadata> {
        let started = options.started_at_ms.unwrap_or_else(now_ms);
        fs::create_dir_all(self.session_dir(&session.id))?;
        fs::create_dir_all(self.run_dir(&session.id, &options.run_id))?;
        write_json(
            &self.session_json_path(&session.id),
            &json!({
                "schema_version": "openagent.session.v1",
                "session_id": session.id,
                "workspace": session.directory.to_string_lossy(),
                "status": session_status_str(&session.status),
                "created_at_ms": started,
                "updated_at_ms": started,
                "active_run_id": options.run_id,
            }),
        )?;
        write_json(
            &self.run_json_path(&session.id, &options.run_id),
            &json!({
                "schema_version": "openagent.run.v1",
                "session_id": session.id,
                "run_id": options.run_id,
                "trace_id": options.trace_id,
                "agent_name": options.agent_name,
                "model_id": options.model_id,
                "provider_id": options.provider_id,
                "permission": options.permission,
                "max_steps": options.max_steps,
                "status": "running",
                "started_at_ms": started,
                "ended_at_ms": Value::Null,
            }),
        )?;
        let metadata = self.metadata(&session.id, &options.run_id);
        session.metadata.insert(
            SESSION_STORE_METADATA_KEY.to_string(),
            serde_json::to_value(&metadata)?,
        );
        append_jsonl(
            &self.index_path(),
            &json!({"event": "run.started", "session_id": session.id, "run_id": options.run_id, "timestamp_ms": started}),
        )?;
        self.record_event(
            &session.id,
            &options.run_id,
            "run.started",
            SessionEventOptions {
                kind: "run".to_string(),
                attributes: BTreeMap::from([
                    ("agent_name".to_string(), json!(options.agent_name)),
                    ("model_id".to_string(), json!(options.model_id)),
                    ("provider_id".to_string(), json!(options.provider_id)),
                    ("permission".to_string(), json!(options.permission)),
                    ("max_steps".to_string(), json!(options.max_steps)),
                ]),
                ..SessionEventOptions::default()
            },
        )?;
        self.save_state(session, Some(&options.run_id))?;
        Ok(metadata)
    }

    pub fn finish_run(
        &self,
        session: &Session,
        run_id: &str,
        status: &str,
        steps: u64,
        finish_reason: Option<&str>,
        error: Option<&str>,
    ) -> SessionResult<()> {
        let ended = now_ms();
        self.record_event(
            &session.id,
            run_id,
            if status == "completed" {
                "run.finished"
            } else {
                "run.failed"
            },
            SessionEventOptions {
                kind: "run".to_string(),
                status: if status == "completed" { "ok" } else { "error" }.to_string(),
                attributes: BTreeMap::from([
                    ("status".to_string(), json!(status)),
                    ("steps".to_string(), json!(steps)),
                    ("finish_reason".to_string(), json!(finish_reason)),
                    ("error".to_string(), json!(error)),
                ]),
                ..SessionEventOptions::default()
            },
        )?;
        let run_path = self.run_json_path(&session.id, run_id);
        let mut run_record = read_json_object(&run_path)?.unwrap_or_default();
        run_record.insert("status".to_string(), json!(status));
        run_record.insert("ended_at_ms".to_string(), json!(ended));
        run_record.insert("steps".to_string(), json!(steps));
        run_record.insert("finish_reason".to_string(), json!(finish_reason));
        run_record.insert("error".to_string(), json!(error));
        let started = run_record
            .get("started_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or(ended);
        run_record.insert(
            "duration_ms".to_string(),
            json!(ended.saturating_sub(started)),
        );
        write_json(&run_path, &Value::Object(run_record))?;
        self.save_state(session, Some(run_id))
    }
}
