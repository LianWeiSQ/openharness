#[must_use]
pub fn emit_app_bridge_events(
    events: &[Value],
    output_format: &str,
    verbose: bool,
) -> CliRunResult {
    let mut result = CliRunResult::default();
    let mut printed_answer = false;
    let mut status = "failed".to_string();
    let mut final_answer = String::new();

    for event in events {
        if output_format == "json" {
            result.stdout.push_str(&stable_json_dumps(event));
            result.stdout.push('\n');
        } else if emit_text_event(event, verbose, &mut result.stdout, &mut result.stderr) {
            printed_answer = true;
        }

        let method = event_method(event);
        let params = event_params(event);
        if matches!(
            method.as_str(),
            "turn/completed" | "turn/failed" | "turn/interrupted"
        ) {
            let default_status = match method.as_str() {
                "turn/completed" => "completed",
                "turn/interrupted" => "interrupted",
                _ => "failed",
            };
            status = params
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or(default_status)
                .to_string();
            final_answer = params
                .get("final_answer")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
    }

    if output_format == "text" {
        if printed_answer {
            result.stdout.push('\n');
        } else if !final_answer.is_empty() {
            result.stdout.push_str(&final_answer);
            result.stdout.push('\n');
        }
        if status != "completed" {
            result
                .stderr
                .push_str(&format!("OpenAgent client turn failed: {status}\n"));
        }
    }
    result.exit_code = if status == "completed" { 0 } else { 1 };
    result
}
