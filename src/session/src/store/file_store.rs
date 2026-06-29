#[derive(Clone, Debug)]
pub struct FileSessionStore {
    pub root: PathBuf,
}

impl FileSessionStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn from_options(options: Option<&Value>, base_dir: Option<&Path>) -> Option<Self> {
        let raw = options
            .and_then(|value| value.get(SESSION_STORE_METADATA_KEY))
            .cloned()
            .unwrap_or_else(|| json!({}));
        if raw == Value::Bool(false) {
            return None;
        }
        let object = raw.as_object();
        if object
            .and_then(|items| items.get("enabled"))
            .is_some_and(|value| !bool_option(value, true))
        {
            return None;
        }
        let root_raw = object
            .and_then(|items| items.get("root_dir"))
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_SESSION_STORE_ROOT);
        let mut root = PathBuf::from(root_raw);
        if !root.is_absolute() {
            root = base_dir.unwrap_or_else(|| Path::new(".")).join(root);
        }
        Some(Self::new(root))
    }

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

    pub fn append_message(
        &self,
        session: &Session,
        message: &ChatMessage,
        run_id: &str,
        index: u64,
    ) -> SessionResult<()> {
        let message_id = message_id(message, index);
        let timestamp_ms = now_ms();
        append_jsonl(
            &self.transcript_path(&session.id),
            &json!({
                "schema_version": "openagent.message.v1",
                "message_id": message_id,
                "session_id": session.id,
                "run_id": run_id,
                "index": index,
                "role": message.role,
                "content": message.content,
                "name": message.name,
                "tool_call_id": message.tool_call_id,
                "metadata": message.metadata,
                "timestamp_ms": timestamp_ms,
            }),
        )?;
        self.append_message_v2(session, message, run_id, index, &message_id, timestamp_ms)?;
        self.record_event(
            &session.id,
            run_id,
            "message.appended",
            SessionEventOptions {
                kind: "message".to_string(),
                attributes: BTreeMap::from([
                    ("message_id".to_string(), json!(message_id)),
                    ("index".to_string(), json!(index)),
                    ("role".to_string(), json!(message.role.clone())),
                    (
                        "content_chars".to_string(),
                        json!(message.content.chars().count()),
                    ),
                    ("tool_call_id".to_string(), json!(message.tool_call_id)),
                ]),
                ..SessionEventOptions::default()
            },
        )?;
        Ok(())
    }

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

    pub fn list_messages_with_parts(
        &self,
        session_id: &str,
        limit: Option<usize>,
        before: Option<&str>,
    ) -> SessionResult<Vec<MessageWithParts>> {
        let mut messages = self.load_v2_messages_from_transcript(session_id)?;
        if messages.is_empty() {
            messages = self.project_legacy_messages_from_transcript(session_id)?;
        }
        if let Some(before) = before.filter(|value| !value.is_empty())
            && let Some(index) = messages
                .iter()
                .position(|message| message.info.id == before)
        {
            messages.truncate(index);
        }
        if let Some(limit) = limit.filter(|value| *value > 0)
            && messages.len() > limit
        {
            messages = messages[messages.len() - limit..].to_vec();
        }
        Ok(messages)
    }

    pub fn get_message_with_parts(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> SessionResult<Option<MessageWithParts>> {
        Ok(self
            .list_messages_with_parts(session_id, None, None)?
            .into_iter()
            .find(|message| message.info.id == message_id))
    }

    pub fn materialized_chat_messages(&self, session: &Session) -> SessionResult<Vec<ChatMessage>> {
        let messages = self.list_messages_with_parts(&session.id, None, None)?;
        if messages.is_empty() {
            return Ok(session.messages.clone());
        }
        Ok(message_parts_to_chat_messages(&messages))
    }

    fn append_message_v2(
        &self,
        session: &Session,
        message: &ChatMessage,
        run_id: &str,
        index: u64,
        message_id: &str,
        timestamp_ms: u64,
    ) -> SessionResult<()> {
        if message.role == Role::Tool {
            return self.append_tool_result_message_v2(
                session,
                message,
                run_id,
                index,
                message_id,
                timestamp_ms,
            );
        }
        let mut metadata = message.metadata.clone();
        if let Some(name) = &message.name {
            metadata.insert("name".to_string(), json!(name));
        }
        if let Some(tool_call_id) = &message.tool_call_id {
            metadata.insert("tool_call_id".to_string(), json!(tool_call_id));
        }
        let info = MessageInfo {
            id: message_id.to_string(),
            session_id: session.id.clone(),
            role: message.role.clone(),
            created_at_ms: timestamp_ms,
            run_id: Some(run_id.to_string()),
            step_index: step_index_from_metadata(&metadata),
            status: MessageStatus::Completed,
            metadata,
        };
        append_jsonl(
            &self.transcript_path(&session.id),
            &StoredMessageV2 {
                schema_version: "openagent.message.v2".to_string(),
                index,
                info: info.clone(),
            },
        )?;
        for part in message_parts_from_chat_message(
            &session.id,
            run_id,
            message_id,
            timestamp_ms,
            message,
            index,
        ) {
            self.append_message_part_v2(&session.id, part)?;
        }
        Ok(())
    }

    fn append_tool_result_message_v2(
        &self,
        session: &Session,
        message: &ChatMessage,
        run_id: &str,
        index: u64,
        message_id: &str,
        timestamp_ms: u64,
    ) -> SessionResult<()> {
        let target_message_id = message
            .metadata
            .get("assistant_message_id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| {
                self.find_assistant_message_for_tool_result(
                    &session.id,
                    message.tool_call_id.as_deref(),
                )
                .ok()
                .flatten()
            });
        let Some(target_message_id) = target_message_id else {
            let info = MessageInfo {
                id: message_id.to_string(),
                session_id: session.id.clone(),
                role: Role::Tool,
                created_at_ms: timestamp_ms,
                run_id: Some(run_id.to_string()),
                step_index: step_index_from_metadata(&message.metadata),
                status: if message_tool_error(message).is_some() {
                    MessageStatus::Error
                } else {
                    MessageStatus::Completed
                },
                metadata: message.metadata.clone(),
            };
            append_jsonl(
                &self.transcript_path(&session.id),
                &StoredMessageV2 {
                    schema_version: "openagent.message.v2".to_string(),
                    index,
                    info,
                },
            )?;
            let part = tool_result_part_from_message(
                &session.id,
                run_id,
                message_id,
                1,
                timestamp_ms,
                message,
                index,
            );
            return self.append_message_part_v2(&session.id, part);
        };
        let part = tool_result_part_from_message(
            &session.id,
            run_id,
            &target_message_id,
            self.next_message_part_seq(&session.id, &target_message_id)?,
            timestamp_ms,
            message,
            index,
        );
        self.append_message_part_v2(&session.id, part)
    }

    fn append_message_part_v2(&self, session_id: &str, part: MessagePart) -> SessionResult<()> {
        append_jsonl(
            &self.transcript_path(session_id),
            &StoredMessagePartV2 {
                schema_version: "openagent.message_part.v2".to_string(),
                part,
            },
        )
    }

    fn load_v2_messages_from_transcript(
        &self,
        session_id: &str,
    ) -> SessionResult<Vec<MessageWithParts>> {
        let mut messages = Vec::<MessageWithParts>::new();
        let mut index_by_id = BTreeMap::<String, usize>::new();
        for value in read_jsonl(&self.transcript_path(session_id))? {
            match value.get("schema_version").and_then(Value::as_str) {
                Some("openagent.message.v2") => {
                    let record: StoredMessageV2 = serde_json::from_value(value)?;
                    index_by_id.insert(record.info.id.clone(), messages.len());
                    messages.push(MessageWithParts {
                        info: record.info,
                        parts: Vec::new(),
                    });
                }
                Some("openagent.message_part.v2") => {
                    let record: StoredMessagePartV2 = serde_json::from_value(value)?;
                    if let Some(index) = index_by_id.get(&record.part.message_id).copied() {
                        messages[index].parts.push(record.part);
                    }
                }
                _ => {}
            }
        }
        for message in &mut messages {
            message.parts.sort_by_key(|part| part.seq);
        }
        Ok(messages)
    }

    fn project_legacy_messages_from_transcript(
        &self,
        session_id: &str,
    ) -> SessionResult<Vec<MessageWithParts>> {
        let mut messages = Vec::new();
        for (position, value) in read_jsonl(&self.transcript_path(session_id))?
            .into_iter()
            .enumerate()
        {
            if value.get("schema_version").and_then(Value::as_str)
                == Some("openagent.message_part.v2")
            {
                continue;
            }
            let index = value
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(position as u64);
            let message = serde_json::from_value::<ChatMessage>(value.clone())
                .ok()
                .or_else(|| {
                    serde_json::from_value::<StoredMessage>(value.clone())
                        .ok()
                        .map(chat_message_from_stored)
                });
            let Some(message) = message else {
                continue;
            };
            let message_id = value
                .get("message_id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| stable_message_id(index));
            let run_id = value
                .get("run_id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let timestamp_ms = value
                .get("timestamp_ms")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let mut metadata = message.metadata.clone();
            metadata
                .entry("message_id".to_string())
                .or_insert_with(|| json!(message_id.clone()));
            let info = MessageInfo {
                id: message_id.clone(),
                session_id: session_id.to_string(),
                role: message.role.clone(),
                created_at_ms: timestamp_ms,
                run_id: run_id.clone(),
                step_index: step_index_from_metadata(&metadata),
                status: if message.role == Role::Tool && message_tool_error(&message).is_some() {
                    MessageStatus::Error
                } else {
                    MessageStatus::Completed
                },
                metadata,
            };
            let run_id_ref = run_id.as_deref().unwrap_or_default();
            let parts = if message.role == Role::Tool {
                vec![tool_result_part_from_message(
                    session_id,
                    run_id_ref,
                    &message_id,
                    1,
                    timestamp_ms,
                    &message,
                    index,
                )]
            } else {
                message_parts_from_chat_message(
                    session_id,
                    run_id_ref,
                    &message_id,
                    timestamp_ms,
                    &message,
                    index,
                )
            };
            messages.push(MessageWithParts { info, parts });
        }
        Ok(messages)
    }

    fn find_assistant_message_for_tool_result(
        &self,
        session_id: &str,
        tool_call_id: Option<&str>,
    ) -> SessionResult<Option<String>> {
        let mut last_assistant = None;
        let mut tool_call_messages = BTreeMap::<String, String>::new();
        for value in read_jsonl(&self.transcript_path(session_id))? {
            match value.get("schema_version").and_then(Value::as_str) {
                Some("openagent.message.v2") => {
                    let record: StoredMessageV2 = serde_json::from_value(value)?;
                    if record.info.role == Role::Assistant {
                        last_assistant = Some(record.info.id);
                    }
                }
                Some("openagent.message_part.v2") => {
                    let record: StoredMessagePartV2 = serde_json::from_value(value)?;
                    if record.part.kind == MessagePartKind::Tool
                        && let Some(call_id) = tool_call_id_from_part(&record.part)
                    {
                        tool_call_messages.insert(call_id, record.part.message_id);
                    }
                }
                _ => {}
            }
        }
        if let Some(tool_call_id) = tool_call_id
            && let Some(message_id) = tool_call_messages.get(tool_call_id)
        {
            return Ok(Some(message_id.clone()));
        }
        Ok(last_assistant)
    }

    fn next_message_part_seq(&self, session_id: &str, message_id: &str) -> SessionResult<u64> {
        let mut max_seq = 0;
        for value in read_jsonl(&self.transcript_path(session_id))? {
            if value.get("schema_version").and_then(Value::as_str)
                != Some("openagent.message_part.v2")
            {
                continue;
            }
            let record: StoredMessagePartV2 = serde_json::from_value(value)?;
            if record.part.message_id == message_id {
                max_seq = max_seq.max(record.part.seq);
            }
        }
        Ok(max_seq + 1)
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

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(session_id)
    }

    fn run_dir(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.session_dir(session_id).join("runs").join(run_id)
    }

    fn session_json_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("session.json")
    }

    fn transcript_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("transcript.jsonl")
    }

    fn state_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("state.latest.json")
    }

    fn run_json_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("run.json")
    }

    fn events_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("events.jsonl")
    }

    fn parts_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("parts.jsonl")
    }

    fn summary_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("summary.json")
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.jsonl")
    }
}
