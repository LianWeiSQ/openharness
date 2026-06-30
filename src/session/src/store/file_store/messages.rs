impl FileSessionStore {
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
}
