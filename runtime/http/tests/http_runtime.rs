use std::{
    error::Error,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use openagent_app_server_client::{RemoteAuth, RemoteRuntimeClient};
use openagent_http_runtime::http_runtime_fixture;
use serde_json::Value;

type FakeProviderServer = (u16, thread::JoinHandle<()>, Arc<Mutex<Vec<String>>>);

#[test]
fn http_runtime_fixture_matches_legacy_oracle() -> Result<(), Box<dyn Error>> {
    let fixture = read_fixture()?;
    assert_eq!(http_runtime_fixture(), fixture);
    Ok(())
}

#[test]
fn binary_health_json_smoke_matches_docker_contract() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_openagent-http-runtime"))
        .arg("--health-json")
        .output()?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        payload,
        section(&read_fixture()?, "docker")["expected_stdout_json"]
    );
    Ok(())
}

#[test]
fn dockerfile_matches_smoke_contract() -> Result<(), Box<dyn Error>> {
    let dockerfile = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Dockerfile.openagent-http-runtime"),
    )?;
    let lines = dockerfile
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(Value::from)
        .collect::<Vec<_>>();
    assert_eq!(
        Value::Array(lines),
        section(&read_fixture()?, "docker")["dockerfile"]
    );
    Ok(())
}

#[test]
fn app_bridge_http_routes_cover_static_sse_auth_and_tui_control() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-routes")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let unauthorized = http_request(port, "GET", "/api/health", &[], "")?;
    assert!(unauthorized.starts_with("HTTP/1.1 401"));
    assert!(unauthorized.contains("WWW-Authenticate: Bearer"));

    let basic = http_request(
        port,
        "GET",
        "/api/health",
        &[("Authorization", "Basic b3BlbmFnZW50OnBhc3M=")],
        "",
    )?;
    assert_eq!(json_body(&basic)?["ok"], true);

    let index = authorized_request(port, "GET", "/", "", true)?;
    assert!(index.contains("content-type: text/html"));
    assert!(index.contains("OpenAgent"));

    let created = json_body(&authorized_request(
        port,
        "POST",
        "/api/sessions",
        &format!("{{\"cwd\":\"{}\"}}", workspace.to_string_lossy()),
        false,
    )?)?;
    let session_id = created["session_id"].as_str().expect("session id");
    let started = json_body(&authorized_request(
        port,
        "POST",
        &format!("/api/sessions/{session_id}/turns"),
        "{\"input\":\"hello over bridge\"}",
        false,
    )?)?;
    let turn_id = started["turn_id"].as_str().expect("turn id");

    let turn_events = authorized_request(
        port,
        "GET",
        &format!("/api/turns/{turn_id}/events"),
        "",
        true,
    )?;
    assert!(turn_events.contains("content-type: text/event-stream"));
    assert!(turn_events.contains("event: item/agentMessage/delta"));
    assert!(turn_events.contains("event: turn/completed"));

    let global_events = authorized_request(port, "GET", "/api/events?last_event_id=0", "", true)?;
    assert!(global_events.contains("event: turn/completed"));

    let interrupted = json_body(&authorized_request(
        port,
        "POST",
        &format!("/api/turns/{turn_id}/interrupt"),
        "",
        false,
    )?)?;
    assert_eq!(interrupted["status"], "interrupted");

    let queued = json_body(&authorized_request(
        port,
        "POST",
        "/tui/append-prompt",
        "{\"text\":\"queued prompt\"}",
        false,
    )?)?;
    assert_eq!(queued["queued"], true);
    let next = json_body(&authorized_request(
        port,
        "GET",
        "/tui/control/next",
        "",
        false,
    )?)?;
    assert_eq!(next["path"], "/tui/append-prompt");
    assert_eq!(next["body"]["text"], "queued prompt");

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_round_trips_tui_approval() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-client-approval")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(
        &session_id,
        "run approved command",
        serde_json::json!({
            "permission": "PLAN_ONLY",
            "tool_call": {
                "call_id": "call_bash",
                "name": "bash",
                "input": {"command": "printf approved"}
            }
        }),
    )?;
    assert_eq!(started["status"], "waiting_approval");
    let approval = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["method"] == "turn/approval_requested")
        .and_then(|event| event["params"]["approval"].as_object())
        .cloned()
        .expect("approval");
    let mut response = Value::Object(approval);
    response["action"] = Value::String("allow".to_string());
    response["scope"] = Value::String("once".to_string());

    let resolved = client.respond_approval(&response)?;
    let events = resolved["events"].as_array().expect("resolved events");

    assert!(events.iter().any(|event| {
        event["method"] == "item/toolCall/completed" && event["params"]["output"] == "approved"
    }));
    assert!(events.iter().any(|event| {
        event["method"] == "turn/completed" && event["params"]["status"] == "completed"
    }));

    let global_events = client.global_events(0)?;
    assert!(
        global_events
            .iter()
            .any(|event| event["method"] == "turn/approval_resolved")
    );

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_manages_session_lifecycle() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-session-lifecycle")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let renamed =
        client.update_session(&session_id, serde_json::json!({"title": "Alpha Session"}))?;
    assert_eq!(renamed["session"]["title"], "Alpha Session");

    let search = client.search_sessions("Alpha")?;
    assert_eq!(search.len(), 1);
    assert_eq!(search[0]["session_id"], session_id);

    let child_id = client.create_session(&workspace, Some(&session_id))?;
    let children = client.children(&session_id)?;
    assert!(
        children
            .iter()
            .any(|child| child["session_id"] == child_id && child["forked_from"] == session_id)
    );

    let share = client.share_session(&session_id)?;
    assert_eq!(share["shared"], true);
    assert!(
        share["url"]
            .as_str()
            .unwrap_or_default()
            .starts_with("openagent://share/")
    );
    let unshare = client.unshare_session(&session_id)?;
    assert_eq!(unshare["shared"], false);

    let compact = client.compact_session(&session_id)?;
    assert_eq!(compact["status"], "compacted");
    assert!(compact["summary"]["summary"].as_str().is_some());

    let archived = client.update_session(&session_id, serde_json::json!({"archived": true}))?;
    assert_eq!(archived["session"]["archived"], true);

    let deleted_child = client.delete_session(&child_id)?;
    assert_eq!(deleted_child["removed"], true);
    let deleted_parent = client.delete_session(&session_id)?;
    assert_eq!(deleted_parent["removed"], true);

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_reads_session_transcript() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-session-transcript")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(&session_id, "hello transcript", serde_json::json!({}))?;
    assert_eq!(started["status"], "completed");

    let transcript = client.session_messages(&session_id, Some(2))?;
    let messages = transcript["messages"].as_array().expect("messages");
    let messages_v2 = transcript["messages_v2"].as_array().expect("messages_v2");

    assert_eq!(transcript["session_id"], session_id);
    assert_eq!(transcript["message_count"], 2);
    assert_eq!(transcript["message_v2_count"], 2);
    assert_eq!(transcript["limit"], 2);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages_v2.len(), 2);
    assert_eq!(messages[0]["index"], 0);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "hello transcript");
    assert_eq!(messages_v2[0]["info"]["role"], "user");
    assert_eq!(messages_v2[0]["parts"][0]["kind"], "text");
    assert_eq!(messages_v2[0]["parts"][0]["content"], "hello transcript");
    assert_eq!(messages[1]["index"], 1);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages_v2[1]["info"]["role"], "assistant");
    assert_eq!(messages_v2[1]["parts"][0]["kind"], "text");
    assert!(
        !messages[1]["content"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .is_empty()
    );

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_tracks_file_diff_undo_and_redo() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-file-diff")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let file_path = workspace.join("notes.txt");

    let write = client.start_turn(
        &session_id,
        "write notes",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_write_notes",
                "name": "write",
                "input": {"file_path": "notes.txt", "content": "alpha\n"}
            }
        }),
    )?;
    assert_eq!(write["status"], "completed");
    assert_eq!(fs::read_to_string(&file_path)?, "alpha\n");

    let diff = client.session_diff(&session_id)?;
    assert_eq!(diff["undo_count"], 1);
    assert_eq!(diff["redo_count"], 0);
    assert!(
        diff["latest"]["diff"]
            .as_str()
            .unwrap_or_default()
            .contains("+alpha")
    );

    let undo = client.undo_session(&session_id)?;
    assert_eq!(undo["status"], "undone");
    assert!(!file_path.exists());
    assert_eq!(undo["redo_count"], 1);

    let redo = client.redo_session(&session_id)?;
    assert_eq!(redo["status"], "redone");
    assert_eq!(fs::read_to_string(&file_path)?, "alpha\n");
    assert_eq!(redo["undo_count"], 1);

    let edited = client.start_turn(
        &session_id,
        "edit notes",
        serde_json::json!({
            "permission": "FULL",
            "tool_calls": [
                {
                    "call_id": "call_read_notes",
                    "name": "read",
                    "input": {"file_path": "notes.txt"}
                },
                {
                    "call_id": "call_edit_notes",
                    "name": "edit",
                    "input": {
                        "file_path": "notes.txt",
                        "old_string": "alpha",
                        "new_string": "beta"
                    }
                }
            ]
        }),
    )?;
    assert_eq!(edited["status"], "completed");
    assert_eq!(fs::read_to_string(&file_path)?, "beta\n");
    assert_eq!(client.session_diff(&session_id)?["undo_count"], 2);

    let edit_undo = client.undo_session(&session_id)?;
    assert_eq!(edit_undo["status"], "undone");
    assert_eq!(fs::read_to_string(&file_path)?, "alpha\n");

    let edit_redo = client.redo_session(&session_id)?;
    assert_eq!(edit_redo["status"], "redone");
    assert_eq!(fs::read_to_string(&file_path)?, "beta\n");

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_controls_model_agent_variant_and_thinking() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-profile")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;

    let models = client.models()?;
    assert!(
        models["models"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(
        models["variants"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == "deep"))
    );

    let agents = client.agents()?;
    assert!(
        agents["agents"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["id"] == "coder"))
    );

    let updated = client.update_session(
        &session_id,
        serde_json::json!({
            "agent": "coder",
            "model": "server-local",
            "variant": "deep",
            "thinking": "high"
        }),
    )?;
    assert_eq!(updated["session"]["metadata"]["agent"], "coder");
    assert_eq!(updated["session"]["metadata"]["variant"], "deep");
    assert_eq!(updated["session"]["metadata"]["thinking"], "high");

    let started = client.start_turn(&session_id, "profile turn", serde_json::json!({}))?;
    let turn_started = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["method"] == "turn/started")
        .expect("turn started event");
    assert_eq!(turn_started["params"]["agent"], "coder");
    assert_eq!(turn_started["params"]["model"], "server-local");
    assert_eq!(turn_started["params"]["variant"], "deep");
    assert_eq!(turn_started["params"]["thinking"], "high");
    assert_eq!(started["turn"]["agent"], "coder");
    assert_eq!(started["turn"]["variant"], "deep");
    assert_eq!(started["turn"]["trace"]["agent"], "coder");
    assert!(
        started["turn"]["usage"]["total_tokens"]
            .as_u64()
            .is_some_and(|value| value > 0)
    );

    let override_started = client.start_turn(
        &session_id,
        "override profile",
        serde_json::json!({"agent": "reviewer", "variant": "fast", "thinking": "low"}),
    )?;
    let override_event = override_started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["method"] == "turn/started")
        .expect("turn started event");
    assert_eq!(override_event["params"]["agent"], "reviewer");
    assert_eq!(override_event["params"]["variant"], "fast");
    assert_eq!(override_event["params"]["thinking"], "low");

    let session = client.get_session(&session_id)?;
    assert_eq!(session["metadata"]["agent"], "reviewer");
    assert_eq!(session["metadata"]["variant"], "fast");

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_uses_real_provider_endpoint_for_plain_turn() -> Result<(), Box<dyn Error>>
{
    let (provider_port, provider_thread) = spawn_fake_openai_responses_provider()?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-real-provider")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let provider_base_url = format!("http://127.0.0.1:{provider_port}/v1");
    let mut server = spawn_runtime_with_env(
        port,
        &workspace,
        &session_root,
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", provider_base_url.as_str()),
            ("OPENAI_WIRE_API", "responses"),
            ("OPENAI_MODEL", "fake-model"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(&session_id, "ask provider", serde_json::json!({}))?;

    assert_eq!(started["status"], "completed");
    assert_eq!(started["turn"]["final_answer"], "real provider answer");
    assert_eq!(started["turn"]["usage"]["input_tokens"], 7);
    assert_eq!(started["turn"]["usage"]["output_tokens"], 3);
    assert!(
        started["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|event| event["method"] == "item/agentMessage/delta"
                && event["params"]["delta"] == "real provider answer")
    );

    let _ = server.kill();
    let _ = provider_thread.join();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_continues_provider_after_tool_call() -> Result<(), Box<dyn Error>> {
    let first = serde_json::json!({
        "id": "resp_tool_call",
        "output": [{
            "type": "function_call",
            "call_id": "call_read_notes",
            "name": "read",
            "arguments": "{\"file_path\":\"notes.txt\"}"
        }],
        "usage": {"input_tokens": 5, "output_tokens": 1}
    });
    let second = serde_json::json!({
        "id": "resp_final",
        "output_text": "tool result says alpha",
        "usage": {"input_tokens": 9, "output_tokens": 4}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![first, second])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-provider-tool-loop")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    fs::write(workspace.join("notes.txt"), "alpha\n")?;
    let provider_base_url = format!("http://127.0.0.1:{provider_port}/v1");
    let mut server = spawn_runtime_with_env(
        port,
        &workspace,
        &session_root,
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", provider_base_url.as_str()),
            ("OPENAI_WIRE_API", "responses"),
            ("OPENAI_MODEL", "fake-model"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(&session_id, "read notes", serde_json::json!({}))?;

    assert_eq!(started["status"], "completed");
    assert_eq!(started["turn"]["final_answer"], "tool result says alpha");
    assert_eq!(started["turn"]["usage"]["input_tokens"], 14);
    assert_eq!(started["turn"]["usage"]["output_tokens"], 5);
    assert_eq!(started["turn"]["usage"]["tool_calls"], 1);
    let events = started["events"].as_array().expect("events");
    assert!(events.iter().any(|event| {
        event["method"] == "item/toolCall/completed"
            && event["params"]["call_id"] == "call_read_notes"
            && event["params"]["output"]
                .as_str()
                .is_some_and(|value| value.contains("alpha"))
    }));
    assert!(events.iter().any(|event| {
        event["method"] == "item/agentMessage/delta"
            && event["params"]["delta"] == "tool result says alpha"
    }));

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("function_call_output"));
    assert!(requests[1].contains("alpha"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_resumes_provider_after_question_reply() -> Result<(), Box<dyn Error>> {
    let first = serde_json::json!({
        "id": "resp_question",
        "output": [{
            "type": "function_call",
            "call_id": "call_question",
            "name": "question",
            "arguments": "{\"questions\":[{\"header\":\"Confirm\",\"question\":\"Proceed?\",\"multiple\":false,\"options\":[{\"label\":\"yes\",\"description\":\"Continue\"},{\"label\":\"no\",\"description\":\"Stop\"}]}]}"
        }],
        "usage": {"input_tokens": 4, "output_tokens": 1}
    });
    let second = serde_json::json!({
        "id": "resp_final",
        "output_text": "continuing after yes",
        "usage": {"input_tokens": 8, "output_tokens": 3}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![first, second])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-provider-question-resume")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let provider_base_url = format!("http://127.0.0.1:{provider_port}/v1");
    let mut server = spawn_runtime_with_env(
        port,
        &workspace,
        &session_root,
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", provider_base_url.as_str()),
            ("OPENAI_WIRE_API", "responses"),
            ("OPENAI_MODEL", "fake-model"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(&session_id, "ask a question", serde_json::json!({}))?;
    assert_eq!(started["status"], "waiting_question");
    let question = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["method"] == "item/question/requested")
        .and_then(|event| event["params"]["event"].as_object())
        .cloned()
        .expect("question event");
    let mut response = Value::Object(question);
    response["answers"] = serde_json::json!([["yes"]]);

    let resolved = client.respond_question(&response)?;
    assert_eq!(resolved["status"], "completed");
    assert_eq!(
        resolved["turn"]["final_answer"],
        serde_json::json!("continuing after yes")
    );
    let events = resolved["events"].as_array().expect("resolved events");
    assert!(events.iter().any(|event| {
        event["method"] == "item/toolCall/completed"
            && event["params"]["name"] == "question"
            && event["params"]["output"]
                .as_str()
                .is_some_and(|value| value.contains("yes"))
    }));
    assert!(events.iter().any(|event| {
        event["method"] == "turn/completed"
            && event["params"]["final_answer"] == "continuing after yes"
    }));
    let session = client.get_session(&session_id)?;
    assert!(session["metadata"]["pending_question"].is_null());
    assert!(session["metadata"]["pending_provider_turn"].is_null());

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("function_call_output"));
    assert!(requests[1].contains("yes"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn remote_runtime_client_resumes_provider_after_approval_allow() -> Result<(), Box<dyn Error>> {
    let first = serde_json::json!({
        "id": "resp_approval",
        "output": [{
            "type": "function_call",
            "call_id": "call_bash",
            "name": "bash",
            "arguments": "{\"command\":\"printf approved\"}"
        }],
        "usage": {"input_tokens": 6, "output_tokens": 1}
    });
    let second = serde_json::json!({
        "id": "resp_final",
        "output_text": "approval flow completed",
        "usage": {"input_tokens": 10, "output_tokens": 4}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence(vec![first, second])?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-provider-approval-resume")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let provider_base_url = format!("http://127.0.0.1:{provider_port}/v1");
    let mut server = spawn_runtime_with_env(
        port,
        &workspace,
        &session_root,
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", provider_base_url.as_str()),
            ("OPENAI_WIRE_API", "responses"),
            ("OPENAI_MODEL", "fake-model"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = client.start_turn(
        &session_id,
        "run command",
        serde_json::json!({"permission": "PLAN_ONLY"}),
    )?;
    assert_eq!(started["status"], "waiting_approval");
    let approval = started["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["method"] == "turn/approval_requested")
        .and_then(|event| event["params"]["approval"].as_object())
        .cloned()
        .expect("approval event");
    let mut response = Value::Object(approval);
    response["action"] = Value::String("allow".to_string());
    response["scope"] = Value::String("once".to_string());

    let resolved = client.respond_approval(&response)?;
    assert_eq!(resolved["status"], "completed");
    assert_eq!(
        resolved["turn"]["final_answer"],
        serde_json::json!("approval flow completed")
    );
    let events = resolved["events"].as_array().expect("resolved events");
    assert!(events.iter().any(|event| {
        event["method"] == "item/toolCall/completed"
            && event["params"]["name"] == "bash"
            && event["params"]["output"] == "approved"
    }));
    assert!(events.iter().any(|event| {
        event["method"] == "turn/completed"
            && event["params"]["final_answer"] == "approval flow completed"
    }));
    let session = client.get_session(&session_id)?;
    assert!(session["metadata"]["pending_approval"].is_null());
    assert!(session["metadata"]["pending_provider_turn"].is_null());

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("function_call_output"));
    assert!(requests[1].contains("approved"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn live_sse_tails_interaction_resolved_events_before_provider_final() -> Result<(), Box<dyn Error>>
{
    run_live_interaction_resume_case("question")?;
    run_live_interaction_resume_case("approval")?;
    Ok(())
}

#[test]
fn global_sse_live_tails_provider_stream_delta_before_completion() -> Result<(), Box<dyn Error>> {
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_streaming_provider()?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-provider-live-stream")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let provider_base_url = format!("http://127.0.0.1:{provider_port}/v1");
    let mut server = spawn_runtime_with_env(
        port,
        &workspace,
        &session_root,
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", provider_base_url.as_str()),
            ("OPENAI_WIRE_API", "responses"),
            ("OPENAI_MODEL", "fake-model"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let live = thread::spawn(move || {
        http_request(
            port,
            "GET",
            "/api/events?last_event_id=0&live_timeout_ms=700",
            &[
                ("Authorization", "Bearer secret"),
                ("Accept", "text/event-stream"),
            ],
            "",
        )
        .map_err(|error| error.to_string())
    });
    thread::sleep(Duration::from_millis(150));

    let turn_session_id = session_id.clone();
    let turn = thread::spawn(move || {
        let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
            .with_auth(RemoteAuth::bearer("secret"));
        client
            .start_turn(
                &turn_session_id,
                "stream from provider",
                serde_json::json!({}),
            )
            .map_err(|error| error.to_string())
    });

    let live_response = live
        .join()
        .map_err(|_| "live sse thread panicked".to_string())?
        .map_err(|error| format!("live sse request failed: {error}"))?;
    assert!(live_response.contains("event: item/agentMessage/delta"));
    assert!(live_response.contains("streamed "));
    assert!(!live_response.contains("event: turn/completed"));

    let started = turn
        .join()
        .map_err(|_| "turn thread panicked".to_string())?
        .map_err(|error| format!("turn failed: {error}"))?;
    assert_eq!(started["status"], "completed");
    assert_eq!(started["turn"]["final_answer"], "streamed answer");
    assert_eq!(started["turn"]["usage"]["input_tokens"], 11);
    assert_eq!(started["turn"]["usage"]["output_tokens"], 2);

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("accept: text/event-stream")
    );
    assert!(requests[0].contains("\"stream\":true"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn global_sse_live_tails_provider_tool_events_before_final_answer() -> Result<(), Box<dyn Error>> {
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_streaming_tool_then_delayed_final_provider()?;
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-provider-tool-live-stream")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    fs::write(workspace.join("notes.txt"), "alpha\n")?;
    let provider_base_url = format!("http://127.0.0.1:{provider_port}/v1");
    let mut server = spawn_runtime_with_env(
        port,
        &workspace,
        &session_root,
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", provider_base_url.as_str()),
            ("OPENAI_WIRE_API", "responses"),
            ("OPENAI_MODEL", "fake-model"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let live = thread::spawn(move || {
        http_request(
            port,
            "GET",
            "/api/events?last_event_id=0&live_timeout_ms=800",
            &[
                ("Authorization", "Bearer secret"),
                ("Accept", "text/event-stream"),
            ],
            "",
        )
        .map_err(|error| error.to_string())
    });
    thread::sleep(Duration::from_millis(150));

    let turn_session_id = session_id.clone();
    let turn = thread::spawn(move || {
        let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
            .with_auth(RemoteAuth::bearer("secret"));
        client
            .start_turn(
                &turn_session_id,
                "read notes with live tool events",
                serde_json::json!({}),
            )
            .map_err(|error| error.to_string())
    });

    let live_response = live
        .join()
        .map_err(|_| "live sse thread panicked".to_string())?
        .map_err(|error| format!("live sse request failed: {error}"))?;
    assert!(live_response.contains("event: item/toolCall/started"));
    assert!(live_response.contains("event: item/toolCall/completed"));
    assert!(live_response.contains("call_live_read"));
    assert!(live_response.contains("alpha"));
    assert!(!live_response.contains("event: turn/completed"));

    let started = turn
        .join()
        .map_err(|_| "turn thread panicked".to_string())?
        .map_err(|error| format!("turn failed: {error}"))?;
    assert_eq!(started["status"], "completed");
    assert_eq!(started["turn"]["final_answer"], "tool final answer");

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("function_call_output"));
    assert!(requests[1].contains("alpha"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

#[test]
fn global_sse_live_tails_events_after_connection() -> Result<(), Box<dyn Error>> {
    let port = free_port()?;
    let temp = temp_dir("openagent-http-runtime-live-sse")?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let mut server = spawn_runtime(port, &workspace, &session_root)?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let live = thread::spawn(move || {
        http_request(
            port,
            "GET",
            "/api/events?last_event_id=0&live_timeout_ms=5000",
            &[
                ("Authorization", "Bearer secret"),
                ("Accept", "text/event-stream"),
            ],
            "",
        )
        .map_err(|error| error.to_string())
    });
    thread::sleep(Duration::from_millis(150));

    let started = client.start_turn(
        &session_id,
        "write notes",
        serde_json::json!({
            "permission": "FULL",
            "tool_call": {
                "call_id": "call_live_write",
                "name": "write",
                "input": {"file_path": "live.txt", "content": "live\n"}
            }
        }),
    )?;
    assert_eq!(started["status"], "completed");

    let live_response = live
        .join()
        .map_err(|_| "live sse thread panicked".to_string())?
        .map_err(|error| format!("live sse request failed: {error}"))?;
    assert!(live_response.contains("content-type: text/event-stream"));
    assert!(
        !live_response
            .to_ascii_lowercase()
            .contains("content-length")
    );
    assert!(live_response.contains("event: item/toolCall/completed"));
    assert!(live_response.contains("event: turn/completed"));

    let _ = server.kill();
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

fn read_fixture() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/rust_rewrite/http_runtime.json");
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn section(fixture: &Value, name: &str) -> Value {
    fixture.get(name).cloned().unwrap_or(Value::Null)
}

fn run_live_interaction_resume_case(kind: &str) -> Result<(), Box<dyn Error>> {
    let first = match kind {
        "question" => serde_json::json!({
            "id": "resp_question_live",
            "output": [{
                "type": "function_call",
                "call_id": "call_question_live",
                "name": "question",
                "arguments": "{\"questions\":[{\"header\":\"Confirm\",\"question\":\"Proceed?\",\"multiple\":false,\"options\":[{\"label\":\"yes\",\"description\":\"Continue\"},{\"label\":\"no\",\"description\":\"Stop\"}]}]}"
            }],
            "usage": {"input_tokens": 4, "output_tokens": 1}
        }),
        "approval" => serde_json::json!({
            "id": "resp_approval_live",
            "output": [{
                "type": "function_call",
                "call_id": "call_bash_live",
                "name": "bash",
                "arguments": "{\"command\":\"printf approved\"}"
            }],
            "usage": {"input_tokens": 6, "output_tokens": 1}
        }),
        other => return Err(format!("unsupported interaction case: {other}").into()),
    };
    let final_answer = format!("{kind} final answer");
    let second = serde_json::json!({
        "id": format!("resp_{kind}_final"),
        "output_text": final_answer.clone(),
        "usage": {"input_tokens": 9, "output_tokens": 3}
    });
    let (provider_port, provider_thread, provider_requests) =
        spawn_fake_openai_responses_provider_sequence_with_delays(vec![
            (first, 0),
            (second, 1500),
        ])?;
    let port = free_port()?;
    let temp = temp_dir(&format!("openagent-http-runtime-live-{kind}-resume"))?;
    let workspace = temp.join("workspace");
    let session_root = temp.join("sessions");
    fs::create_dir_all(&workspace)?;
    let provider_base_url = format!("http://127.0.0.1:{provider_port}/v1");
    let mut server = spawn_runtime_with_env(
        port,
        &workspace,
        &session_root,
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", provider_base_url.as_str()),
            ("OPENAI_WIRE_API", "responses"),
            ("OPENAI_MODEL", "fake-model"),
        ],
    )?;
    wait_for_server(port)?;

    let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
        .with_auth(RemoteAuth::bearer("secret"));
    let session_id = client.create_session(&workspace, None)?;
    let started = if kind == "approval" {
        client.start_turn(
            &session_id,
            "run command with approval",
            serde_json::json!({"permission": "PLAN_ONLY"}),
        )?
    } else {
        client.start_turn(&session_id, "ask a question", serde_json::json!({}))?
    };
    assert_eq!(
        started["status"],
        if kind == "approval" {
            "waiting_approval"
        } else {
            "waiting_question"
        }
    );
    let mut response = if kind == "approval" {
        Value::Object(
            started["events"]
                .as_array()
                .expect("events")
                .iter()
                .find(|event| event["method"] == "turn/approval_requested")
                .and_then(|event| event["params"]["approval"].as_object())
                .cloned()
                .expect("approval event"),
        )
    } else {
        Value::Object(
            started["events"]
                .as_array()
                .expect("events")
                .iter()
                .find(|event| event["method"] == "item/question/requested")
                .and_then(|event| event["params"]["event"].as_object())
                .cloned()
                .expect("question event"),
        )
    };
    if kind == "approval" {
        response["action"] = Value::String("allow".to_string());
        response["scope"] = Value::String("once".to_string());
    } else {
        response["answers"] = serde_json::json!([["yes"]]);
    }
    let request_id = response["request_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let live = thread::spawn(move || {
        http_request(
            port,
            "GET",
            "/api/events?last_event_id=0&live_timeout_ms=800",
            &[
                ("Authorization", "Bearer secret"),
                ("Accept", "text/event-stream"),
            ],
            "",
        )
        .map_err(|error| error.to_string())
    });
    thread::sleep(Duration::from_millis(150));

    let response_for_thread = response.clone();
    let kind_for_thread = kind.to_string();
    let reply = thread::spawn(move || {
        let client = RemoteRuntimeClient::new(format!("http://127.0.0.1:{port}"))
            .with_auth(RemoteAuth::bearer("secret"));
        if kind_for_thread == "approval" {
            client
                .respond_approval(&response_for_thread)
                .map_err(|error| error.to_string())
        } else {
            client
                .respond_question(&response_for_thread)
                .map_err(|error| error.to_string())
        }
    });

    let live_response = live
        .join()
        .map_err(|_| "live sse thread panicked".to_string())?
        .map_err(|error| format!("live sse request failed: {error}"))?;
    if kind == "approval" {
        assert!(live_response.contains("event: turn/approval_resolved"));
        assert!(live_response.contains("running"));
    } else {
        assert!(live_response.contains("event: item/question/resolved"));
        assert!(live_response.contains("answered"));
    }
    assert!(live_response.contains(&request_id));
    assert!(live_response.contains(&session_id));
    assert!(!live_response.contains("event: turn/completed"));

    let resolved = reply
        .join()
        .map_err(|_| "interaction reply thread panicked".to_string())?
        .map_err(|error| format!("interaction reply failed: {error}"))?;
    assert_eq!(resolved["status"], "completed");
    assert_eq!(resolved["turn"]["final_answer"], final_answer);

    let _ = server.kill();
    let _ = provider_thread.join();
    let requests = provider_requests.lock().expect("provider requests");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("function_call_output"));
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

fn free_port() -> Result<u16, Box<dyn Error>> {
    static NEXT_PORT: AtomicU16 = AtomicU16::new(0);
    if NEXT_PORT.load(Ordering::Relaxed) == 0 {
        let seed = 20_000 + (std::process::id() % 20_000) as u16;
        let _ = NEXT_PORT.compare_exchange(0, seed, Ordering::Relaxed, Ordering::Relaxed);
    }
    for _ in 0..10_000 {
        let port = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

fn temp_dir(prefix: &str) -> Result<PathBuf, Box<dyn Error>> {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    let path = std::env::temp_dir().join(format!("{prefix}-{suffix}"));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn spawn_runtime(
    port: u16,
    workspace: &std::path::Path,
    session_root: &std::path::Path,
) -> Result<Child, Box<dyn Error>> {
    spawn_runtime_with_env(port, workspace, session_root, &[])
}

fn spawn_runtime_with_env(
    port: u16,
    workspace: &std::path::Path,
    session_root: &std::path::Path,
    envs: &[(&str, &str)],
) -> Result<Child, Box<dyn Error>> {
    let port = port.to_string();
    let mut command = Command::new(env!("CARGO_BIN_EXE_openagent-http-runtime"));
    command
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port,
            "--workspace",
            workspace.to_str().unwrap_or("."),
            "--session-root",
            session_root.to_str().unwrap_or("."),
            "--auth-token",
            "secret",
            "--username",
            "openagent",
            "--password",
            "pass",
            "--cors-origin",
            "http://client.test",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in envs {
        command.env(key, value);
    }
    Ok(command.spawn()?)
}

fn spawn_fake_openai_responses_provider() -> Result<(u16, thread::JoinHandle<()>), Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    Ok((
        port,
        thread::spawn(move || {
            if let Ok((mut stream, _addr)) = listener.accept() {
                let mut buffer = [0_u8; 8192];
                let _ = stream.read(&mut buffer);
                let body = serde_json::json!({
                    "id": "resp_fake",
                    "output_text": "real provider answer",
                    "usage": {"input_tokens": 7, "output_tokens": 3}
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }),
    ))
}

fn spawn_fake_openai_responses_provider_sequence(
    responses: Vec<Value>,
) -> Result<FakeProviderServer, Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    Ok((
        port,
        thread::spawn(move || {
            for body_value in responses {
                let Ok((mut stream, _addr)) = listener.accept() else {
                    break;
                };
                let mut buffer = [0_u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or_default();
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                if let Some((_, body)) = request.split_once("\r\n\r\n") {
                    if let Ok(mut items) = captured.lock() {
                        items.push(body.to_string());
                    }
                }
                let body = body_value.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }),
        requests,
    ))
}

fn spawn_fake_openai_responses_provider_sequence_with_delays(
    responses: Vec<(Value, u64)>,
) -> Result<FakeProviderServer, Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    Ok((
        port,
        thread::spawn(move || {
            for (body_value, delay_ms) in responses {
                let Ok((mut stream, _addr)) = listener.accept() else {
                    break;
                };
                let mut buffer = [0_u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or_default();
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                if let Some((_, body)) = request.split_once("\r\n\r\n") {
                    if let Ok(mut items) = captured.lock() {
                        items.push(body.to_string());
                    }
                }
                if delay_ms > 0 {
                    thread::sleep(Duration::from_millis(delay_ms));
                }
                let body = body_value.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }),
        requests,
    ))
}

fn spawn_fake_openai_responses_streaming_provider() -> Result<FakeProviderServer, Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    Ok((
        port,
        thread::spawn(move || {
            if let Ok((mut stream, _addr)) = listener.accept() {
                let mut buffer = [0_u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or_default();
                if let Ok(mut items) = captured.lock() {
                    items.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                }
                let headers = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\nconnection: close\r\n\r\n";
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(
                    b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"streamed \"}\n\n",
                );
                let _ = stream.flush();
                thread::sleep(Duration::from_millis(1500));
                let _ = stream.write_all(
                    b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"answer\"}\n\n",
                );
                let _ = stream.write_all(
                    b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":11,\"output_tokens\":2}}}\n\n",
                );
                let _ = stream.write_all(b"data: [DONE]\n\n");
                let _ = stream.flush();
            }
        }),
        requests,
    ))
}

fn spawn_fake_openai_responses_streaming_tool_then_delayed_final_provider()
-> Result<FakeProviderServer, Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    Ok((
        port,
        thread::spawn(move || {
            if let Ok((mut stream, _addr)) = listener.accept() {
                let mut buffer = [0_u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or_default();
                if let Ok(mut items) = captured.lock() {
                    items.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                }
                let headers = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\nconnection: close\r\n\r\n";
                let tool_event = serde_json::json!({
                    "type": "response.output_item.done",
                    "item": {
                        "type": "function_call",
                        "call_id": "call_live_read",
                        "name": "read",
                        "arguments": "{\"file_path\":\"notes.txt\"}",
                    }
                })
                .to_string();
                let usage_event = serde_json::json!({
                    "type": "response.completed",
                    "response": {"usage": {"input_tokens": 5, "output_tokens": 1}}
                })
                .to_string();
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(format!("data: {tool_event}\n\n").as_bytes());
                let _ = stream.write_all(format!("data: {usage_event}\n\n").as_bytes());
                let _ = stream.write_all(b"data: [DONE]\n\n");
                let _ = stream.flush();
            }

            if let Ok((mut stream, _addr)) = listener.accept() {
                let mut buffer = [0_u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or_default();
                if let Ok(mut items) = captured.lock() {
                    items.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                }
                thread::sleep(Duration::from_millis(1500));
                let body = serde_json::json!({
                    "id": "resp_final_after_tool",
                    "output_text": "tool final answer",
                    "usage": {"input_tokens": 12, "output_tokens": 3}
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }),
        requests,
    ))
}

fn wait_for_server(port: u16) -> Result<(), Box<dyn Error>> {
    for _ in 0..100 {
        if authorized_request(port, "GET", "/api/health", "", false).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("server did not start".into())
}

fn authorized_request(
    port: u16,
    method: &str,
    path: &str,
    body: &str,
    raw: bool,
) -> Result<String, Box<dyn Error>> {
    let response = http_request(
        port,
        method,
        path,
        &[("Authorization", "Bearer secret")],
        body,
    )?;
    if raw || response.starts_with("HTTP/1.1 2") {
        return Ok(response);
    }
    Err(format!("request failed: {response}").into())
}

fn http_request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<String, Box<dyn Error>> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if !body.is_empty() {
        request.push_str("Content-Type: application/json\r\n");
    }
    for (key, value) in headers {
        request.push_str(&format!("{key}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn json_body(response: &str) -> Result<Value, Box<dyn Error>> {
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(response);
    Ok(serde_json::from_str(body)?)
}
