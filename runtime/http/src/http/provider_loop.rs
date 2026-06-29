#[derive(Clone, Debug)]
struct RuntimeProviderResult {
    answer: String,
    tool_calls: Vec<ToolCall>,
    usage: Usage,
    source: String,
    finish_reason: String,
}

struct OpenAiRuntimeProviderRequest<'a> {
    provider: &'a str,
    model: &'a str,
    api_key: &'a str,
    base_url: &'a str,
    wire_api: &'a str,
    timeout_s: u64,
    stream: bool,
    messages: &'a [ChatMessage],
    tools: &'a [openagent_protocol::ToolSchema],
}

#[derive(Clone, Debug)]
struct RuntimeProviderLoopCarry {
    answer: String,
    usage: Usage,
    tool_calls: u64,
    next_step: u64,
}

impl Default for RuntimeProviderLoopCarry {
    fn default() -> Self {
        Self {
            answer: String::new(),
            usage: Usage::default(),
            tool_calls: 0,
            next_step: 1,
        }
    }
}

#[derive(Clone, Debug)]
struct RuntimeProviderResume {
    payload: Value,
    carry: RuntimeProviderLoopCarry,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
}

struct RuntimeProviderLoopInput<'a> {
    store: &'a FileSessionStore,
    session: &'a mut Session,
    run_id: &'a str,
    payload: &'a Value,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
    events: Vec<Value>,
    carry: RuntimeProviderLoopCarry,
}

fn provider_turn_result(
    store: &FileSessionStore,
    session: &Session,
    payload: &Value,
    stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
) -> Result<RuntimeProviderResult, String> {
    let provider_raw = payload
        .get("provider")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            session
                .metadata
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| std::env::var("OPENAGENT_PROVIDER").ok())
        .unwrap_or_else(|| "openai".to_string());
    let provider = normalize_provider(Some(&provider_raw))?;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            session
                .metadata
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| std::env::var("OPENAGENT_MODEL").ok())
        .or_else(|| provider_default_model(&provider).ok().flatten())
        .unwrap_or_else(|| "gpt-4o-mini".to_string());
    let env = default_env_mapping(&provider)?;
    let api_key = payload
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| env.get("api_key").and_then(|key| std::env::var(key).ok()))
        .or_else(|| std::env::var("OPENAGENT_API_KEY").ok());
    if provider_requires_api_key(&provider)? && api_key.as_deref().unwrap_or_default().is_empty() {
        return Ok(RuntimeProviderResult {
            answer: format!(
                "Provider `{provider}` is not configured. Set {} or OPENAGENT_API_KEY, then retry this turn.",
                env.get("api_key")
                    .map(String::as_str)
                    .unwrap_or("OPENAI_API_KEY")
            ),
            tool_calls: Vec::new(),
            usage: Usage::default(),
            source: "provider_missing_api_key".to_string(),
            finish_reason: "configuration_required".to_string(),
        });
    }
    let api_key = api_key.unwrap_or_default();
    let base_url = payload
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| env.get("base_url").and_then(|key| std::env::var(key).ok()))
        .or_else(|| provider_default_base_url(&provider).ok().flatten())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let wire_api = payload
        .get("wire_api")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| env.get("wire_api").and_then(|key| std::env::var(key).ok()))
        .unwrap_or_else(|| "responses".to_string());
    let timeout = payload
        .get("timeout_s")
        .and_then(Value::as_u64)
        .unwrap_or(60);
    let stream = provider_streaming_enabled_for_turn(payload);
    let tools = Toolkit::with_builtins().get_all_tools("local");
    let provider_messages = store
        .materialized_chat_messages(session)
        .unwrap_or_else(|_| session.messages.clone());
    call_openai_compatible_provider_for_runtime(
        OpenAiRuntimeProviderRequest {
            provider: &provider,
            model: &model,
            api_key: &api_key,
            base_url: &base_url,
            wire_api: &wire_api,
            timeout_s: timeout,
            stream,
            messages: &provider_messages,
            tools: &tools,
        },
        stream_sink,
    )
}

fn call_openai_compatible_provider_for_runtime(
    request: OpenAiRuntimeProviderRequest<'_>,
    mut stream_sink: Option<&mut dyn FnMut(&ProviderStreamEvent)>,
) -> Result<RuntimeProviderResult, String> {
    let OpenAiRuntimeProviderRequest {
        provider,
        model,
        api_key,
        base_url,
        wire_api,
        timeout_s,
        stream,
        messages,
        tools,
    } = request;
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(timeout_s.max(1)))
        .build()
        .map_err(|error| error.to_string())?;
    let mut config = OpenAiLanguageModelConfig::new(api_key, model);
    config.provider_id = provider.to_string();
    config.base_url = base_url.to_string();
    config.wire_api = wire_api.to_string();
    let (endpoint, payload) = if wire_api == "chat" {
        let mut payload =
            build_openai_chat_payload(&config, None, messages, tools, None, None, None);
        if let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(stream));
        }
        (join_url(base_url, "chat/completions"), payload)
    } else {
        let mut payload =
            build_openai_responses_payload(&config, None, messages, tools, None, None);
        if let Some(object) = payload.as_object_mut() {
            object.insert("stream".to_string(), json!(stream));
        }
        (join_url(base_url, "responses"), payload)
    };
    let mut request = client
        .post(endpoint)
        .bearer_auth(api_key)
        .header("content-type", "application/json");
    if stream {
        request = request.header("accept", "text/event-stream");
    }
    let response = request
        .json(&payload)
        .send()
        .map_err(|error| format!("provider request failed: {error}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if stream && content_type.contains("text/event-stream") {
        if !status.is_success() {
            let raw = response
                .text()
                .map_err(|error| format!("provider response read failed: {error}"))?;
            return Err(format!(
                "provider returned HTTP {}: {}",
                status.as_u16(),
                summarize_http_error_body(&raw, &content_type)
            ));
        }
        let mut chunks = Vec::new();
        read_sse_json_values_stream(response, |chunk| {
            if let Some(event) = openai_stream_text_delta(wire_api, &chunk)
                && let Some(sink) = stream_sink.as_deref_mut()
            {
                sink(&event);
            }
            chunks.push(chunk);
            Ok(())
        })?;
        let events = if wire_api == "chat" {
            normalize_openai_chat_sse_chunks(&chunks)
        } else {
            normalize_openai_responses_stream_events(&chunks)
        };
        return Ok(provider_events_to_runtime_result(
            &events,
            format!("{provider}:{wire_api}:stream"),
            None,
        ));
    }
    let raw = response
        .text()
        .map_err(|error| format!("provider response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "provider returned HTTP {}: {}",
            status.as_u16(),
            summarize_http_error_body(&raw, &content_type)
        ));
    }
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("provider response was not JSON: {error}"))?;
    if wire_api == "chat" {
        Ok(openai_chat_response_to_runtime_result(
            &value,
            format!("{provider}:chat"),
        ))
    } else {
        let events = normalize_openai_responses_response(&value);
        Ok(provider_events_to_runtime_result(
            &events,
            format!("{provider}:responses"),
            Some(&value),
        ))
    }
}

fn read_sse_json_values_stream<R, F>(mut reader: R, mut on_value: F) -> Result<(), String>
where
    R: Read,
    F: FnMut(Value) -> Result<(), String>,
{
    let mut raw = String::new();
    let mut buffer = [0_u8; 4096];
    let mut saw_done = false;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(_error) if saw_done => break,
            Err(error) => return Err(format!("provider SSE read failed: {error}")),
        };
        if read == 0 {
            break;
        }
        raw.push_str(&String::from_utf8_lossy(&buffer[..read]));
        while let Some(index) = sse_frame_end(&raw) {
            let frame = raw[..index].to_string();
            let drain_to = if raw[index..].starts_with("\r\n\r\n") {
                index + 4
            } else {
                index + 2
            };
            raw.drain(..drain_to);
            if sse_frame_is_done(&frame) {
                saw_done = true;
            }
            if let Some(value) = parse_sse_frame_json(&frame)? {
                on_value(value)?;
            }
        }
    }
    if !raw.trim().is_empty()
        && let Some(value) = parse_sse_frame_json(&raw)?
    {
        on_value(value)?;
    }
    Ok(())
}

fn sse_frame_is_done(frame: &str) -> bool {
    frame.lines().any(|line| {
        let line = line.trim_end_matches('\r');
        line.strip_prefix("data:")
            .map(str::trim)
            .is_some_and(|data| data == "[DONE]")
    })
}

fn sse_frame_end(raw: &str) -> Option<usize> {
    match (raw.find("\r\n\r\n"), raw.find("\n\n")) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(index), None) | (None, Some(index)) => Some(index),
        (None, None) => None,
    }
}

fn parse_sse_frame_json(frame: &str) -> Result<Option<Value>, String> {
    let mut data_lines = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }
    if data_lines.is_empty() {
        return Ok(None);
    }
    let data = data_lines.join("\n");
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .map_err(|error| format!("provider SSE data was not JSON: {error}"))
}

fn openai_stream_text_delta(wire_api: &str, chunk: &Value) -> Option<ProviderStreamEvent> {
    let text = if wire_api == "chat" {
        chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("content"))
            .or_else(|| {
                chunk
                    .get("choices")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(|choice| choice.get("text"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else if matches!(
        chunk.get("type").and_then(Value::as_str),
        Some("response.output_text.delta" | "response.refusal.delta")
    ) {
        chunk
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else {
        ""
    };
    (!text.is_empty()).then(|| ProviderStreamEvent::TextDelta {
        text: text.to_string(),
    })
}

fn openai_chat_response_to_runtime_result(value: &Value, source: String) -> RuntimeProviderResult {
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));
    let answer = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let function = item.get("function")?;
            Some(ToolCall {
                call_id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("chat_tool_call_{index}"), str::to_string),
                name: function
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: parse_tool_arguments(
                    function
                        .get("arguments")
                        .unwrap_or(&Value::String(String::new())),
                ),
            })
        })
        .collect::<Vec<_>>();
    let usage = usage_from_provider_json(value.get("usage"));
    RuntimeProviderResult {
        answer: if answer.is_empty() && tool_calls.is_empty() {
            stable_json_dumps(value)
        } else {
            answer
        },
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or(if tool_calls.is_empty() {
                "stop"
            } else {
                "tool_call"
            })
            .to_string(),
        tool_calls,
        usage,
        source,
    }
}

fn provider_events_to_runtime_result(
    events: &[ProviderStreamEvent],
    source: String,
    fallback: Option<&Value>,
) -> RuntimeProviderResult {
    let mut answer = String::new();
    let mut tool_calls = Vec::new();
    let mut usage = Usage::default();
    let mut finish_reason = "stop".to_string();
    for event in events {
        match event {
            ProviderStreamEvent::TextDelta { text } => answer.push_str(text),
            ProviderStreamEvent::ToolCall {
                call_id,
                name,
                input,
            } => tool_calls.push(ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                input: input.clone(),
            }),
            ProviderStreamEvent::Finish {
                usage: item,
                finish_reason: reason,
            } => {
                usage = item.clone();
                finish_reason = reason.clone();
            }
        }
    }
    if answer.is_empty()
        && tool_calls.is_empty()
        && let Some(value) = fallback
    {
        answer = stable_json_dumps(value);
    }
    RuntimeProviderResult {
        answer,
        tool_calls,
        usage,
        source,
        finish_reason,
    }
}

fn provider_max_steps(payload: &Value) -> u64 {
    payload
        .get("max_steps")
        .or_else(|| payload.get("maxSteps"))
        .and_then(Value::as_u64)
        .unwrap_or(4)
        .clamp(1, 16)
}

fn add_usage(total: &mut Usage, item: &Usage) {
    total.input_tokens = total.input_tokens.saturating_add(item.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(item.output_tokens);
    total.cost += item.cost;
}

fn provider_resume_payload(payload: &Value) -> Value {
    let mut value = payload.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("input");
        object.remove("message");
        object.remove("tool_call");
        object.remove("tool_calls");
        object.remove("api_key");
    }
    value
}

fn store_pending_provider_turn(
    session: &mut Session,
    payload: &Value,
    carry: &RuntimeProviderLoopCarry,
    permission_ruleset: PermissionRuleset,
    skip_permissions: bool,
) {
    session.metadata.insert(
        "pending_provider_turn".to_string(),
        json!({
            "payload": provider_resume_payload(payload),
            "answer": carry.answer.clone(),
            "usage": carry.usage.clone(),
            "tool_calls": carry.tool_calls,
            "next_step": carry.next_step,
            "permission": permission_ruleset.as_str(),
            "skip_permissions": skip_permissions,
        }),
    );
}

fn take_pending_provider_turn(session: &mut Session) -> Option<RuntimeProviderResume> {
    let pending = session.metadata.remove("pending_provider_turn")?;
    let permission_raw = pending
        .get("permission")
        .and_then(Value::as_str)
        .unwrap_or("FULL");
    Some(RuntimeProviderResume {
        payload: pending.get("payload").cloned().unwrap_or_else(|| json!({})),
        carry: RuntimeProviderLoopCarry {
            answer: pending
                .get("answer")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            usage: usage_from_provider_json(pending.get("usage")),
            tool_calls: pending
                .get("tool_calls")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            next_step: pending
                .get("next_step")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .max(1),
        },
        permission_ruleset: parse_permission_ruleset(permission_raw).ok()?,
        skip_permissions: pending
            .get("skip_permissions")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn usage_from_provider_json(value: Option<&Value>) -> Usage {
    let input_tokens = value
        .and_then(|item| {
            item.get("input_tokens")
                .or_else(|| item.get("prompt_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or_default();
    let output_tokens = value
        .and_then(|item| {
            item.get("output_tokens")
                .or_else(|| item.get("completion_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or_default();
    Usage {
        input_tokens,
        output_tokens,
        cost: 0.0,
    }
}

fn usage_value_from_provider(
    usage: &Usage,
    tool_calls: u64,
    fallback_input: &str,
    fallback_output: &str,
) -> Value {
    let fallback = usage_payload(fallback_input, fallback_output, tool_calls);
    let input_tokens = if usage.input_tokens == 0 {
        fallback["input_tokens"].as_u64().unwrap_or_default()
    } else {
        usage.input_tokens
    };
    let output_tokens = if usage.output_tokens == 0 {
        fallback["output_tokens"].as_u64().unwrap_or_default()
    } else {
        usage.output_tokens
    };
    let tool_tokens = tool_calls.saturating_mul(16);
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "tool_tokens": tool_tokens,
        "total_tokens": input_tokens + output_tokens + tool_tokens,
        "tool_calls": tool_calls,
        "cost": usage.cost,
        "estimated": usage.input_tokens == 0 && usage.output_tokens == 0,
    })
}

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn runtime_chat_message(role: Role, content: String) -> ChatMessage {
    ChatMessage {
        role,
        content,
        name: None,
        tool_call_id: None,
        metadata: BTreeMap::new(),
    }
}

fn runtime_message_id(index: u64) -> String {
    format!("msg_{index}")
}

fn latest_assistant_message_id_for_tool(session: &Session, tool_call: &ToolCall) -> Option<String> {
    session.messages.iter().rev().find_map(|message| {
        if message.role != Role::Assistant {
            return None;
        }
        let message_id = message
            .metadata
            .get("message_id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let has_call = message
            .metadata
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| {
                calls.iter().any(|call| {
                    call.get("call_id")
                        .or_else(|| call.get("id"))
                        .and_then(Value::as_str)
                        == Some(tool_call.call_id.as_str())
                })
            });
        if has_call { message_id } else { None }
    })
}

fn assistant_message_for_provider_step(content: String, tool_calls: &[ToolCall]) -> ChatMessage {
    let mut message = runtime_chat_message(Role::Assistant, content);
    if !tool_calls.is_empty() {
        message.metadata.insert(
            "tool_calls".to_string(),
            Value::Array(tool_calls.iter().map(openai_tool_call_value).collect()),
        );
    }
    message
}

fn openai_tool_call_value(call: &ToolCall) -> Value {
    json!({
        "id": call.call_id.clone(),
        "call_id": call.call_id.clone(),
        "type": "function",
        "function": {
            "name": call.name.clone(),
            "arguments": stable_json_dumps(&call.input),
        },
        "name": call.name.clone(),
        "input": call.input.clone(),
    })
}

fn run_provider_loop(input: RuntimeProviderLoopInput<'_>) -> Result<Value, String> {
    let RuntimeProviderLoopInput {
        store,
        session,
        run_id,
        payload,
        permission_ruleset,
        skip_permissions,
        mut events,
        mut carry,
    } = input;
    let max_steps = provider_max_steps(payload);
    let toolkit = Toolkit::with_builtins();
    let mut ctx = ToolContext::new(&session.directory)
        .with_session_id(session.id.clone())
        .with_permission_ruleset(permission_ruleset.clone())
        .with_dangerously_skip_permissions(skip_permissions);
    if let Some(answers) = payload
        .get("question_answers")
        .or_else(|| payload.get("answers"))
        .and_then(question_answers_from_json)
    {
        ctx.set_question_answers(answers);
    }

    let mut persisted_events = 0;
    append_unpersisted_app_events(
        &store.root,
        &session.id,
        run_id,
        &events,
        &mut persisted_events,
    );
    while carry.next_step <= max_steps {
        let step = carry.next_step;
        let mut streamed_text = false;
        let session_id = session.id.clone();
        let root = store.root.clone();
        let mut on_provider_stream = |event: &ProviderStreamEvent| {
            if let ProviderStreamEvent::TextDelta { text } = event
                && !text.is_empty()
            {
                streamed_text = true;
                events.push(json!({
                    "method": "item/agentMessage/delta",
                    "params": {
                        "thread_id": session_id.clone(),
                        "session_id": session_id.clone(),
                        "turn_id": run_id,
                        "run_id": run_id,
                        "step": step,
                        "event": {"id": format!("assistant_{step}"), "text": text.clone()},
                        "delta": text.clone(),
                    }
                }));
                append_unpersisted_app_events(
                    &root,
                    &session_id,
                    run_id,
                    &events,
                    &mut persisted_events,
                );
            }
        };
        let provider_result =
            provider_turn_result(store, session, payload, Some(&mut on_provider_stream))?;
        add_usage(&mut carry.usage, &provider_result.usage);
        if provider_result.source == "provider_missing_api_key" {
            events.push(json!({
                "method": "runtime/warning",
                "params": {
                    "session_id": session.id.clone(),
                    "turn_id": run_id,
                    "message": provider_result.answer.clone(),
                    "code": "provider_missing_api_key",
                }
            }));
        }
        if !provider_result.answer.is_empty() {
            carry.answer.push_str(&provider_result.answer);
            if !streamed_text {
                events.push(json!({
                    "method": "item/agentMessage/delta",
                    "params": {
                        "thread_id": session.id.clone(),
                        "session_id": session.id.clone(),
                        "turn_id": run_id,
                        "run_id": run_id,
                        "step": step,
                        "event": {"id": format!("assistant_{step}"), "text": provider_result.answer.clone()},
                        "delta": provider_result.answer.clone(),
                    }
                }));
            }
            let _ = store.append_part(
                &session.id,
                run_id,
                "text",
                SessionPartOptions {
                    attributes: BTreeMap::from([
                        ("role".to_string(), json!("assistant")),
                        (
                            "chars".to_string(),
                            json!(provider_result.answer.chars().count()),
                        ),
                    ]),
                    step_index: Some(step),
                    ..SessionPartOptions::default()
                },
            );
        }

        let assistant_index = session.messages.len() as u64;
        let assistant_message_id = runtime_message_id(assistant_index);
        let mut assistant = assistant_message_for_provider_step(
            provider_result.answer.clone(),
            &provider_result.tool_calls,
        );
        assistant
            .metadata
            .insert("message_id".to_string(), json!(assistant_message_id));
        assistant.metadata.insert("step".to_string(), json!(step));
        session.add(assistant.clone());
        let _ = store.append_message(session, &assistant, run_id, assistant_index);

        if provider_result.tool_calls.is_empty() {
            return finish_provider_loop(
                store,
                session,
                run_id,
                events,
                &mut persisted_events,
                carry,
                &provider_result.finish_reason,
            );
        }

        let resume_carry = RuntimeProviderLoopCarry {
            next_step: step.saturating_add(1),
            ..carry.clone()
        };
        for tool_call in &provider_result.tool_calls {
            carry.tool_calls = carry.tool_calls.saturating_add(1);
            let pending_carry = RuntimeProviderLoopCarry {
                tool_calls: carry.tool_calls,
                next_step: step.saturating_add(1),
                ..resume_carry.clone()
            };
            if let Some(paused) = execute_provider_tool_call(
                store,
                session,
                run_id,
                payload,
                step,
                tool_call,
                &toolkit,
                &mut ctx,
                &permission_ruleset,
                skip_permissions,
                &pending_carry,
                &mut events,
                &mut persisted_events,
            )? {
                return Ok(paused);
            }
        }

        carry.next_step = step.saturating_add(1);
    }

    session.status = SessionStatus::Idle;
    let _ = store.finish_run(
        session,
        run_id,
        "failed",
        max_steps,
        Some("max_steps"),
        Some("agent loop exceeded max_steps"),
    );
    let usage = usage_value_from_provider(
        &carry.usage,
        carry.tool_calls,
        &latest_user_message(session),
        &carry.answer,
    );
    let trace = trace_payload(session, run_id, carry.tool_calls);
    events.push(json!({
        "method": "turn/failed",
        "params": {
            "session_id": session.id.clone(),
            "turn_id": run_id,
            "status": "failed",
            "error": "agent loop exceeded max_steps",
            "usage": usage,
            "trace": trace,
        }
    }));
    append_unpersisted_app_events(
        &store.root,
        &session.id,
        run_id,
        &events,
        &mut persisted_events,
    );
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "status": "failed",
        "events": events,
    }))
}

#[allow(clippy::too_many_arguments)]
fn execute_provider_tool_call(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    payload: &Value,
    step: u64,
    tool_call: &ToolCall,
    toolkit: &Toolkit,
    ctx: &mut ToolContext,
    permission_ruleset: &PermissionRuleset,
    skip_permissions: bool,
    pending_carry: &RuntimeProviderLoopCarry,
    events: &mut Vec<Value>,
    persisted_events: &mut usize,
) -> Result<Option<Value>, String> {
    events.push(json!({
        "method": "item/toolCall/started",
        "params": {
            "session_id": session.id.clone(),
            "turn_id": run_id,
            "run_id": run_id,
            "step": step,
            "call_id": tool_call.call_id.clone(),
            "name": tool_call.name.clone(),
            "input": tool_call.input.clone(),
        }
    }));
    append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
    let _ = store.record_event(
        &session.id,
        run_id,
        "tool.call.started",
        SessionEventOptions {
            kind: "tool".to_string(),
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("input".to_string(), tool_call.input.clone()),
                ("step".to_string(), json!(step)),
            ]),
            ..SessionEventOptions::default()
        },
    );

    if tool_call.name == "question" && ctx.question_answers.is_none() {
        let question = question_payload_for_tool_call(session, run_id, step, tool_call);
        session.status = SessionStatus::Paused;
        session
            .metadata
            .insert("pending_question".to_string(), question.clone());
        session.metadata.remove("pending_question_response");
        store_pending_provider_turn(
            session,
            payload,
            pending_carry,
            permission_ruleset.clone(),
            skip_permissions,
        );
        let _ = store.record_event(
            &session.id,
            run_id,
            "question.requested",
            SessionEventOptions {
                kind: "question".to_string(),
                attributes: BTreeMap::from([
                    ("call_id".to_string(), json!(tool_call.call_id.clone())),
                    (
                        "questions".to_string(),
                        tool_call
                            .input
                            .get("questions")
                            .cloned()
                            .unwrap_or_else(|| json!([])),
                    ),
                ]),
                ..SessionEventOptions::default()
            },
        );
        if let Some(message_id) = latest_assistant_message_id_for_tool(session, tool_call) {
            let _ = store.append_part(
                &session.id,
                run_id,
                "question",
                SessionPartOptions {
                    message_id: Some(message_id),
                    content: Some(json!({
                        "call_id": tool_call.call_id.clone(),
                        "name": tool_call.name.clone(),
                        "questions": tool_call.input.get("questions").cloned().unwrap_or_else(|| json!([])),
                        "status": "pending",
                    })),
                    attributes: BTreeMap::from([
                        ("call_id".to_string(), json!(tool_call.call_id.clone())),
                        ("name".to_string(), json!(tool_call.name.clone())),
                    ]),
                    step_index: Some(step),
                    status: "pending".to_string(),
                    ..SessionPartOptions::default()
                },
            );
        }
        let _ = store.save_state(session, Some(run_id));
        events.push(json!({
            "method": "item/question/requested",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "status": "waiting_question",
                "event": question,
            }
        }));
        append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
        return Ok(Some(json!({
            "session_id": session.id,
            "turn_id": run_id,
            "status": "waiting_question",
            "events": events,
        })));
    }

    let change_before = capture_file_change_before(session, tool_call);
    let mut tool_result = toolkit.execute(
        &tool_call.name,
        tool_call.input.clone(),
        &tool_call.call_id,
        ctx,
    );
    if tool_result
        .metadata
        .get("requires_approval")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut approval =
            approval_payload_for_tool_call(session, run_id, step, tool_call, &tool_result.metadata);
        if let Some(preview) = change_before
            .as_ref()
            .and_then(|before| file_change_preview(before, tool_call))
            && let Some(object) = approval.as_object_mut()
        {
            object.insert("preview".to_string(), preview);
        }
        session.status = SessionStatus::Paused;
        session
            .metadata
            .insert("pending_approval".to_string(), approval.clone());
        session.metadata.remove("pending_approval_response");
        store_pending_provider_turn(
            session,
            payload,
            pending_carry,
            permission_ruleset.clone(),
            skip_permissions,
        );
        let _ = store.record_event(
            &session.id,
            run_id,
            "approval.requested",
            SessionEventOptions {
                kind: "approval".to_string(),
                attributes: BTreeMap::from([
                    ("call_id".to_string(), json!(tool_call.call_id.clone())),
                    ("name".to_string(), json!(tool_call.name.clone())),
                    ("approval".to_string(), approval.clone()),
                ]),
                ..SessionEventOptions::default()
            },
        );
        if let Some(message_id) = latest_assistant_message_id_for_tool(session, tool_call) {
            let _ = store.append_part(
                &session.id,
                run_id,
                "approval",
                SessionPartOptions {
                    message_id: Some(message_id),
                    content: Some(json!({
                        "call_id": tool_call.call_id.clone(),
                        "name": tool_call.name.clone(),
                        "approval": approval.clone(),
                        "status": "pending",
                    })),
                    attributes: BTreeMap::from([
                        ("call_id".to_string(), json!(tool_call.call_id.clone())),
                        ("name".to_string(), json!(tool_call.name.clone())),
                    ]),
                    step_index: Some(step),
                    status: "pending".to_string(),
                    ..SessionPartOptions::default()
                },
            );
        }
        let _ = store.save_state(session, Some(run_id));
        events.push(json!({
            "method": "turn/approval_requested",
            "params": {
                "session_id": session.id.clone(),
                "turn_id": run_id,
                "status": "waiting_approval",
                "approval": approval,
            }
        }));
        append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
        return Ok(Some(json!({
            "session_id": session.id,
            "turn_id": run_id,
            "status": "waiting_approval",
            "events": events,
        })));
    }

    append_completed_tool_result(
        store,
        session,
        run_id,
        step,
        tool_call,
        change_before,
        &mut tool_result,
        events,
    )?;
    append_unpersisted_app_events(&store.root, &session.id, run_id, events, persisted_events);
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn append_completed_tool_result(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    step: u64,
    tool_call: &ToolCall,
    change_before: Option<FileChangeBefore>,
    tool_result: &mut ToolResult,
    events: &mut Vec<Value>,
) -> Result<(), String> {
    let failed = tool_result.error.is_some();
    let patch = complete_file_change(
        store,
        session,
        run_id,
        tool_call,
        change_before,
        tool_result,
    );
    if let Some(change) = patch.as_ref() {
        tool_result
            .metadata
            .insert("patch".to_string(), public_file_change(change));
        tool_result.metadata.insert(
            "patch_id".to_string(),
            change.get("id").cloned().unwrap_or(Value::Null),
        );
        tool_result.metadata.insert(
            "diff".to_string(),
            change.get("diff").cloned().unwrap_or(Value::Null),
        );
    }
    events.push(json!({
        "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
        "params": {
            "session_id": session.id.clone(),
            "turn_id": run_id,
            "run_id": run_id,
            "step": step,
            "call_id": tool_call.call_id.clone(),
            "name": tool_call.name.clone(),
            "output": tool_result.output.clone(),
            "error": tool_result.error.clone(),
            "metadata": tool_result.metadata.clone(),
        }
    }));
    if let Some(change) = patch.as_ref() {
        events.push(patch_detected_event(session, run_id, change));
    }
    append_tool_result_to_session(store, session, run_id, step, tool_call, tool_result)
}

fn append_tool_result_to_session(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    step: u64,
    tool_call: &ToolCall,
    tool_result: &ToolResult,
) -> Result<(), String> {
    let failed = tool_result.error.is_some();
    let _ = store.record_event(
        &session.id,
        run_id,
        if failed {
            "tool.call.failed"
        } else {
            "tool.call.finished"
        },
        SessionEventOptions {
            kind: "tool".to_string(),
            status: if failed {
                "error".to_string()
            } else {
                "ok".to_string()
            },
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("error".to_string(), json!(tool_result.error.clone())),
                ("metadata".to_string(), json!(tool_result.metadata.clone())),
                ("step".to_string(), json!(step)),
            ]),
            ..SessionEventOptions::default()
        },
    );
    let _ = store.append_part(
        &session.id,
        run_id,
        "tool_result",
        SessionPartOptions {
            attributes: BTreeMap::from([
                ("call_id".to_string(), json!(tool_call.call_id.clone())),
                ("name".to_string(), json!(tool_call.name.clone())),
                ("failed".to_string(), json!(failed)),
            ]),
            step_index: Some(step),
            ..SessionPartOptions::default()
        },
    );
    let mut tool_message = runtime_chat_message(
        Role::Tool,
        tool_result.error.as_ref().map_or_else(
            || tool_result.output.clone(),
            |error| format!("Tool failed: {error}"),
        ),
    );
    tool_message.name = Some(tool_call.name.clone());
    tool_message.tool_call_id = Some(tool_call.call_id.clone());
    tool_message
        .metadata
        .insert("tool_result".to_string(), json!(tool_result));
    tool_message
        .metadata
        .insert("step".to_string(), json!(step));
    if let Some(message_id) = latest_assistant_message_id_for_tool(session, tool_call) {
        tool_message
            .metadata
            .insert("assistant_message_id".to_string(), json!(message_id));
    }
    let tool_index = session.messages.len() as u64;
    session.add(tool_message.clone());
    store
        .append_message(session, &tool_message, run_id, tool_index)
        .map_err(|error| format!("failed to record tool message: {error}"))
}

fn finish_provider_loop(
    store: &FileSessionStore,
    session: &mut Session,
    run_id: &str,
    mut events: Vec<Value>,
    persisted_events: &mut usize,
    carry: RuntimeProviderLoopCarry,
    finish_reason: &str,
) -> Result<Value, String> {
    session.status = SessionStatus::Idle;
    session.metadata.remove("pending_provider_turn");
    let steps = carry.next_step.max(1);
    let _ = store.finish_run(
        session,
        run_id,
        "completed",
        steps,
        Some(finish_reason),
        None,
    );
    let usage = usage_value_from_provider(
        &carry.usage,
        carry.tool_calls,
        &latest_user_message(session),
        &carry.answer,
    );
    let trace = trace_payload(session, run_id, carry.tool_calls);
    record_usage_event(store, session, run_id, &usage);
    events.push(json!({
        "method": "turn/completed",
        "params": {
            "thread_id": session.id.clone(),
            "session_id": session.id.clone(),
            "turn_id": run_id,
            "status": "completed",
            "final_answer": carry.answer,
            "usage": usage,
            "trace": trace,
            "finish_reason": finish_reason,
        }
    }));
    append_unpersisted_app_events(&store.root, &session.id, run_id, &events, persisted_events);
    Ok(json!({
        "session_id": session.id,
        "turn_id": run_id,
        "status": "completed",
        "turn": {
            "id": run_id,
            "session_id": session.id,
            "status": "completed",
            "final_answer": events.last().and_then(|event| event.get("params")).and_then(|params| params.get("final_answer")).cloned().unwrap_or_else(|| json!("")),
            "agent": session_text_metadata(session, "agent", "server"),
            "model": session_text_metadata(session, "model", &default_model_id()),
            "variant": session_text_metadata(session, "variant", "default"),
            "thinking": session_text_metadata(session, "thinking", "medium"),
            "usage": usage,
            "trace": trace,
        },
        "events": events
    }))
}
