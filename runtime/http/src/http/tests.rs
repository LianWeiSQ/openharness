#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_command_boundary() {
        assert_eq!(crate_name(), "openagent-http-runtime");
        assert_eq!(command_name(), "openagent-http-runtime");
        assert_eq!(app_server_crate_name(), "openagent-app-server");
    }

    #[test]
    fn app_bridge_permission_approval_round_trip_executes_allowed_tool() {
        let root = std::env::temp_dir().join(format!("openagent-http-permission-{}", now_ms()));
        let workspace = root.join("workspace");
        let session_root = root.join("sessions");
        fs::create_dir_all(&workspace).expect("workspace");
        let config = HttpRuntimeConfig {
            serve_static: false,
            workspace: Some(workspace.to_string_lossy().to_string()),
            session_store_root: Some(session_root.to_string_lossy().to_string()),
            ..HttpRuntimeConfig::default()
        };
        let created = create_session_payload(
            &config,
            &stable_json_dumps(&json!({"cwd": workspace.to_string_lossy()})),
        );
        let session_id = created
            .get("session_id")
            .and_then(Value::as_str)
            .expect("session id");
        let started = start_turn_payload(
            &config,
            session_id,
            &stable_json_dumps(&json!({
                "input": "run approved command",
                "permission": "PLAN_ONLY",
                "tool_call": {
                    "call_id": "call_bash",
                    "name": "bash",
                    "input": {"command": "printf approved"}
                }
            })),
        )
        .expect("start turn");
        assert_eq!(started["status"], "waiting_approval");
        let approval = started["events"]
            .as_array()
            .expect("events")
            .iter()
            .find(|event| event["method"] == "turn/approval_requested")
            .and_then(|event| event["params"]["approval"].as_object())
            .cloned()
            .expect("approval");
        let turn_id = approval
            .get("turn_id")
            .and_then(Value::as_str)
            .expect("turn id");
        let request_id = approval
            .get("request_id")
            .and_then(Value::as_str)
            .expect("request id");
        let resolved = respond_approval_payload(
            &config,
            &format!("/api/turns/{turn_id}/approvals/{request_id}"),
            &stable_json_dumps(&json!({"action": "allow", "scope": "once"})),
        )
        .expect("resolve approval");
        let events = resolved["events"].as_array().expect("resolved events");
        assert!(events.iter().any(|event| {
            event["method"] == "item/toolCall/completed" && event["params"]["output"] == "approved"
        }));
        assert!(events.iter().any(|event| {
            event["method"] == "turn/completed" && event["params"]["status"] == "completed"
        }));

        let _ = fs::remove_dir_all(root);
    }
}
