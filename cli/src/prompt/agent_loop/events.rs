fn emit_run_event(
    events: &mut Vec<Value>,
    event: Value,
    event_sink: &mut Option<&mut dyn FnMut(&Value)>,
) {
    if let Some(emit) = event_sink.as_deref_mut() {
        emit(&event);
    }
    events.push(event);
}

fn record_step_finished(
    store: &FileSessionStore,
    session_id: &str,
    run_id: &str,
    step: u64,
    finish_reason: &str,
    tool_calls: u64,
    usage: &Usage,
) {
    let _ = store.record_event(
        session_id,
        run_id,
        "step.finished",
        SessionEventOptions {
            kind: "step".to_string(),
            attributes: BTreeMap::from([
                ("step".to_string(), json!(step)),
                ("finish_reason".to_string(), json!(finish_reason)),
                ("tool_calls".to_string(), json!(tool_calls)),
                ("input_tokens".to_string(), json!(usage.input_tokens)),
                ("output_tokens".to_string(), json!(usage.output_tokens)),
            ]),
            ..SessionEventOptions::default()
        },
    );
}
