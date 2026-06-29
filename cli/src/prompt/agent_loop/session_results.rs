fn append_tool_result_to_session(
    context: &mut PendingResumeContext<'_, '_>,
    step: u64,
    tool_call: &ToolCall,
    tool_result: ToolResult,
) -> Result<(), String> {
    let failed = tool_result.error.is_some();
    emit_run_event(
        context.events,
        json!({
            "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" },
            "params": {
                "session_id": context.session.id.clone(),
                "run_id": context.run_id,
                "step": step,
                "call_id": tool_call.call_id.clone(),
                "name": tool_call.name.clone(),
                "output": tool_result.output.clone(),
                "error": tool_result.error.clone(),
                "metadata": tool_result.metadata.clone(),
            }
        }),
        context.event_sink,
    );
    let _ = context.store.record_event(
        &context.session.id,
        context.run_id,
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
    let _ = context.store.append_part(
        &context.session.id,
        context.run_id,
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
    let mut tool_message = chat_message(
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
    if let Some(message_id) = context
        .session
        .metadata
        .get(if tool_call.name == "question" {
            "pending_question"
        } else {
            "pending_approval"
        })
        .and_then(|value| value.get("assistant_message_id"))
        .and_then(Value::as_str)
    {
        tool_message
            .metadata
            .insert("assistant_message_id".to_string(), json!(message_id));
    }
    tool_message
        .metadata
        .insert("step".to_string(), json!(step));
    let tool_index = context.session.messages.len() as u64;
    context.session.add(tool_message.clone());
    context
        .store
        .append_message(context.session, &tool_message, context.run_id, tool_index)
        .map_err(|error| format!("failed to record resumed tool message: {error}"))
}
