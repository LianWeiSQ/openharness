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
