use super::*;

pub(super) fn approval_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(approval_help());
    }
    match args[0].as_str() {
        "list" | "ls" => pending_request_list(args, "pending_approval"),
        "respond" | "allow" | "approve" => approval_respond(&args[1..], false),
        "reject" | "deny" => approval_respond(&args[1..], true),
        other => err_text(2, format!("unknown approval command: {other}")),
    }
}

pub(super) fn question_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(question_help());
    }
    match args[0].as_str() {
        "list" | "ls" => pending_request_list(args, "pending_question"),
        "reply" | "answer" => question_reply(&args[1..], false),
        "reject" | "dismiss" => question_reply(&args[1..], true),
        other => err_text(2, format!("unknown question command: {other}")),
    }
}

fn pending_request_list(args: &[String], key: &str) -> CliRunResult {
    let root = session_root_from_args(args);
    let Some(session_id) =
        value_for(args, &["--session", "-s"]).or_else(|| latest_session_id(&root))
    else {
        return err_text(1, "No sessions found");
    };
    let store = FileSessionStore::new(root);
    let session = match store.load_session(&session_id) {
        Ok(session) => session,
        Err(error) => return err_text(1, format!("failed to load session: {error}")),
    };
    let pending = session.metadata.get(key).cloned().unwrap_or(Value::Null);
    CliRunResult::ok_json(&json!({
        "session_id": session.id,
        "kind": key.trim_start_matches("pending_"),
        "pending": pending,
    }))
}

fn approval_respond(args: &[String], reject: bool) -> CliRunResult {
    let root = session_root_from_args(args);
    let mut session = match load_response_session(args, &root) {
        Ok(session) => session,
        Err(error) => return err_text(1, error),
    };
    let Some(pending) = session.metadata.get("pending_approval").cloned() else {
        return err_text(1, "No pending approval request in session");
    };
    if let Err(error) = ensure_request_matches(args, &pending) {
        return err_text(2, error);
    }
    let decision = if reject || has_flag(args, &["--reject", "--deny"]) {
        "reject".to_string()
    } else if has_flag(args, &["--always"]) {
        "allow_always".to_string()
    } else {
        value_for(args, &["--decision"]).unwrap_or_else(|| "allow_once".to_string())
    };
    if !matches!(
        decision.as_str(),
        "allow_once" | "allow_always" | "always" | "reject" | "deny"
    ) {
        return err_text(
            2,
            "approval decision must be allow_once, allow_always, or reject",
        );
    }
    let response = json!({
        "request_id": pending.get("request_id").cloned().unwrap_or(Value::Null),
        "decision": decision,
        "note": value_for(args, &["--note"]),
        "updated_at_ms": now_ms_cli(),
    });
    session
        .metadata
        .insert("pending_approval_response".to_string(), response.clone());
    session.status = SessionStatus::Paused;
    let store = FileSessionStore::new(root);
    let run_id = pending.get("run_id").and_then(Value::as_str);
    if let Err(error) = store.save_state(&session, run_id) {
        return err_text(1, format!("failed to save approval response: {error}"));
    }
    CliRunResult::ok_json(&json!({
        "session_id": session.id,
        "queued": true,
        "response": response,
        "next": format!("openagent run --continue --session-root {}", store.root.to_string_lossy()),
    }))
}

fn question_reply(args: &[String], reject: bool) -> CliRunResult {
    let root = session_root_from_args(args);
    let mut session = match load_response_session(args, &root) {
        Ok(session) => session,
        Err(error) => return err_text(1, error),
    };
    let Some(pending) = session.metadata.get("pending_question").cloned() else {
        return err_text(1, "No pending question request in session");
    };
    if let Err(error) = ensure_request_matches(args, &pending) {
        return err_text(2, error);
    }
    let answers = if reject {
        Vec::<Vec<String>>::new()
    } else {
        let mut answers = values_for(args, &["--answer"])
            .into_iter()
            .map(|answer| split_answer_items(&answer))
            .collect::<Vec<_>>();
        if answers.is_empty() {
            let positionals = positional_args(
                args,
                &[
                    "--workspace",
                    "--dir",
                    "--session-root",
                    "--session",
                    "-s",
                    "--request-id",
                    "--format",
                ],
            );
            if !positionals.is_empty() {
                answers.push(vec![positionals.join(" ")]);
            }
        }
        if answers.is_empty() {
            return err_text(2, "question reply requires --answer or answer text");
        }
        answers
    };
    let response = json!({
        "request_id": pending.get("request_id").cloned().unwrap_or(Value::Null),
        "decision": if reject { "reject" } else { "reply" },
        "answers": answers,
        "updated_at_ms": now_ms_cli(),
    });
    session
        .metadata
        .insert("pending_question_response".to_string(), response.clone());
    session.status = SessionStatus::Paused;
    let store = FileSessionStore::new(root);
    let run_id = pending.get("run_id").and_then(Value::as_str);
    if let Err(error) = store.save_state(&session, run_id) {
        return err_text(1, format!("failed to save question response: {error}"));
    }
    CliRunResult::ok_json(&json!({
        "session_id": session.id,
        "queued": true,
        "response": response,
        "next": format!("openagent run --continue --session-root {}", store.root.to_string_lossy()),
    }))
}

fn load_response_session(args: &[String], root: &Path) -> Result<Session, String> {
    let session_id = value_for(args, &["--session", "-s"])
        .or_else(|| latest_session_id(root))
        .ok_or_else(|| "No sessions found".to_string())?;
    if !valid_session_id(&session_id) {
        return Err("Invalid session id".to_string());
    }
    FileSessionStore::new(root)
        .load_session(&session_id)
        .map_err(|error| format!("failed to load session: {error}"))
}

fn ensure_request_matches(args: &[String], pending: &Value) -> Result<(), String> {
    let expected = pending
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let requested = value_for(args, &["--request-id"]).or_else(|| {
        positional_args(
            args,
            &[
                "--workspace",
                "--dir",
                "--session-root",
                "--session",
                "-s",
                "--decision",
                "--note",
                "--answer",
                "--format",
            ],
        )
        .first()
        .cloned()
        .filter(|value| value.starts_with("approval_") || value.starts_with("question_"))
    });
    if let Some(requested) = requested
        && !expected.is_empty()
        && requested != expected
    {
        return Err(format!(
            "request id mismatch: expected {expected}, received {requested}"
        ));
    }
    Ok(())
}
