use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use openagent_protocol::{AgentDescriptor, FanoutBudget, PermissionMode, RunLimits, RunStatus};
use openagent_swarm::{
    A2ARequestConfig, A2ARunner, FunctionRunner, HttpRequestConfig, HttpRunner, ResultPayload,
    RunnerRegistry, SubprocessCommand, SubprocessRunner, SwarmRuntime, TaskConfig,
    build_transport_registry, load_swarm_config_from_str,
};
use serde_json::{Value, json};

#[tokio::test]
async fn function_runtime_dispatches_and_aggregates_results() {
    let mut registry = RunnerRegistry::new();
    registry.register(FunctionRunner::new(
        descriptor("alpha", "function", &["research"]),
        Arc::new(|spec, _ctx| {
            Box::pin(async move {
                ResultPayload::Json(json!({
                    "status": "completed",
                    "summary": format!("{} complete", spec.role),
                    "confidence": 0.82,
                    "usage": {
                        "input_tokens": 4,
                        "output_tokens": 6,
                        "cost": 0.15,
                        "steps": 1,
                        "latency_ms": 11
                    },
                    "metadata": {"source": "function-alpha"}
                }))
            })
        }),
    ));
    registry.register(FunctionRunner::new(
        descriptor("beta", "function", &["research"]),
        Arc::new(|_spec, _ctx| {
            Box::pin(async move {
                ResultPayload::Json(json!({
                    "status": "completed",
                    "summary": "beta complete",
                    "usage": {
                        "input_tokens": 2,
                        "output_tokens": 3,
                        "cost": 0.05,
                        "steps": 2,
                        "latency_ms": 7
                    }
                }))
            })
        }),
    ));

    let runtime = SwarmRuntime::new(registry, FanoutBudget::default());
    let result = runtime
        .run_task(
            &task(
                "function-task",
                "research",
                vec!["alpha".to_string(), "beta".to_string()],
            ),
            Some("function-run".to_string()),
        )
        .await;

    assert_eq!(result.status, "completed");
    assert!(result.summary.contains("[alpha] completed"));
    assert!(result.summary.contains("[beta] completed"));
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.events.len(), 4);
    assert_eq!(result.usage.input_tokens, 6);
    assert_eq!(result.usage.output_tokens, 9);
    assert_eq!(result.usage.steps, 3);
}

#[tokio::test]
async fn function_runner_reports_spec_validation_failures() {
    let mut registry = RunnerRegistry::new();
    registry.register(FunctionRunner::new(
        descriptor("validator", "function", &["review"]),
        Arc::new(|_spec, _ctx| {
            Box::pin(async { ResultPayload::Text("should not run".to_string()) })
        }),
    ));
    let runtime = SwarmRuntime::new(registry, FanoutBudget::default());
    let mut invalid = task("invalid-task", "review", vec!["validator".to_string()]);
    invalid.context = String::new();
    invalid.boundaries = String::new();
    invalid.output_schema = json!({});

    let result = runtime
        .run_task(&invalid, Some("validation-run".to_string()))
        .await;
    let validator = result
        .results
        .get("validator")
        .expect("validator result is recorded");

    assert_eq!(result.status, "failed");
    assert_eq!(validator.status, RunStatus::Failed);
    assert!(validator.summary.contains("context"));
    assert!(validator.summary.contains("boundaries"));
    assert!(validator.summary.contains("output_schema"));
    assert_eq!(
        validator.metadata.get("error_kind"),
        Some(&json!("agent_spec_validation_error"))
    );
}

#[tokio::test]
async fn subprocess_runner_executes_json_worker() {
    let script = r#"printf '%s\n' '{"status":"completed","summary":"subprocess worker","evidence":["objective:Deliver the answer"],"usage":{"input_tokens":5,"output_tokens":7,"cost":0.25,"steps":1,"latency_ms":9},"metadata":{"seen_runner":"subprocess-one"}}'"#;
    let mut registry = RunnerRegistry::new();
    registry.register(SubprocessRunner::new(
        descriptor("subprocess-one", "subprocess", &["worker"]),
        SubprocessCommand {
            argv: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            cwd: None,
            env: BTreeMap::new(),
            timeout_seconds: Some(5.0),
        },
    ));
    let runtime = SwarmRuntime::new(registry, FanoutBudget::default());

    let result = runtime
        .run_task(
            &task(
                "subprocess-task",
                "worker",
                vec!["subprocess-one".to_string()],
            ),
            Some("subprocess-run".to_string()),
        )
        .await;
    let worker = result
        .results
        .get("subprocess-one")
        .expect("subprocess result exists");

    assert_eq!(result.status, "completed");
    assert_eq!(worker.status, RunStatus::Completed);
    assert_eq!(worker.summary, "subprocess worker");
    assert_eq!(worker.evidence, vec!["objective:Deliver the answer"]);
    assert_eq!(worker.usage.input_tokens, 5);
    assert_eq!(worker.metadata.get("stdout_format"), Some(&json!("json")));
    assert_eq!(
        worker.metadata.get("seen_runner"),
        Some(&json!("subprocess-one"))
    );
}

#[tokio::test]
async fn http_runner_posts_payload_and_normalizes_response() {
    let (url, received, handle) = start_json_server(json!({
        "status": "completed",
        "summary": "http complete",
        "usage": {"input_tokens": 1, "output_tokens": 2, "cost": 0.01, "steps": 1, "latency_ms": 3}
    }));
    let mut registry = RunnerRegistry::new();
    registry.register(HttpRunner::new(
        descriptor("http-one", "http", &["web"]),
        HttpRequestConfig {
            url: format!("{url}/run"),
            method: "POST".to_string(),
            headers: BTreeMap::new(),
            timeout_seconds: Some(5.0),
        },
    ));
    let runtime = SwarmRuntime::new(registry, FanoutBudget::default());

    let result = runtime
        .run_task(
            &task("http-task", "web", vec!["http-one".to_string()]),
            Some("http-run".to_string()),
        )
        .await;
    let request = received
        .recv_timeout(Duration::from_secs(5))
        .expect("server captures HTTP request");
    handle.join().expect("server thread joins cleanly");
    let payload: Value = serde_json::from_str(&request.body).expect("HTTP request body is JSON");

    assert_eq!(request.path, "/run");
    assert_eq!(payload["spec"]["role"], "web");
    assert_eq!(payload["runner"]["id"], "http-one");
    assert_eq!(result.status, "completed");
    assert_eq!(result.usage.input_tokens, 1);
    assert_eq!(
        result.results["http-one"].metadata.get("response_format"),
        Some(&json!("json"))
    );
}

#[tokio::test]
async fn a2a_runner_uses_message_send_contract() {
    let (url, received, handle) = start_json_server(json!({
        "status": "completed",
        "summary": "a2a complete",
        "metadata": {"remote": "a2a"}
    }));
    let mut registry = RunnerRegistry::new();
    registry.register(A2ARunner::new(
        descriptor("a2a-one", "a2a", &["delegate"]),
        A2ARequestConfig {
            url: format!("{url}/agent"),
            headers: BTreeMap::new(),
            timeout_seconds: Some(5.0),
        },
    ));
    let runtime = SwarmRuntime::new(registry, FanoutBudget::default());

    let result = runtime
        .run_task(
            &task("a2a-task", "delegate", vec!["a2a-one".to_string()]),
            Some("a2a-run".to_string()),
        )
        .await;
    let request = received
        .recv_timeout(Duration::from_secs(5))
        .expect("server captures A2A request");
    handle.join().expect("server thread joins cleanly");
    let payload: Value = serde_json::from_str(&request.body).expect("A2A request body is JSON");

    assert_eq!(request.path, "/agent/message/send");
    assert_eq!(payload["message"]["role"], "ROLE_USER");
    assert_eq!(
        payload["configuration"]["metadata"]["swarm_runner_id"],
        "a2a-one"
    );
    assert_eq!(result.status, "completed");
    assert_eq!(
        result.results["a2a-one"].metadata.get("remote"),
        Some(&json!("a2a"))
    );
}

#[test]
fn transport_registry_loads_yaml_shape() {
    let raw = r#"
fanout_budget:
  max_concurrent: 1
  max_total_workers: 2
runners:
  sub-one:
    kind: subprocess
    roles: [worker]
    metadata:
      command: [sh, -c, "printf ok"]
tasks:
  demo:
    role: worker
    objective: Demo
    context: Context
    boundaries: Boundaries
    output_schema:
      type: object
    runner_ids: [sub-one]
"#;
    let config = load_swarm_config_from_str(raw).expect("YAML config loads");
    let registry = build_transport_registry(&config).expect("transport registry builds");

    assert_eq!(config.task("demo").expect("task exists").role, "worker");
    assert_eq!(registry.ids(), vec!["sub-one".to_string()]);
}

#[test]
fn cli_run_executes_configured_subprocess_task() {
    let script = r#"printf '%s\n' '{"status":"completed","summary":"cli cli","usage":{"input_tokens":3,"output_tokens":4,"cost":0.02,"steps":1,"latency_ms":5}}'"#;
    let config = json!({
        "fanout_budget": {
            "max_concurrent": 1,
            "max_total_workers": 1
        },
        "runners": {
            "cli-worker": {
                "kind": "subprocess",
                "roles": ["cli"],
                "metadata": {
                    "command": ["sh", "-c", script],
                    "timeout_seconds": 5.0
                }
            }
        },
        "tasks": {
            "cli-task": {
                "role": "cli",
                "objective": "Run through the CLI",
                "context": "CLI context",
                "boundaries": "Return JSON",
                "output_schema": {"type": "object"},
                "runner_ids": ["cli-worker"]
            }
        }
    });
    let path = unique_temp_path("openagent-swarm-cli", "json");
    fs::write(&path, config.to_string()).expect("config file is written");

    let output = Command::new(env!("CARGO_BIN_EXE_openagent-swarm"))
        .args([
            "run",
            path.to_str().expect("temp path is UTF-8"),
            "--task",
            "cli-task",
        ])
        .output()
        .expect("CLI process runs");
    fs::remove_file(&path).expect("temporary config file is removed");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let payload: Value = serde_json::from_str(&stdout).expect("CLI emits JSON");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(payload["status"], "completed");
    assert_eq!(payload["results"]["cli-worker"]["summary"], "cli cli");
    assert_eq!(payload["usage"]["input_tokens"], 3);
}

fn descriptor(id: &str, kind: &str, roles: &[&str]) -> AgentDescriptor {
    AgentDescriptor {
        id: id.to_string(),
        roles: roles.iter().map(|role| (*role).to_string()).collect(),
        tool_groups: Vec::new(),
        model_tier: "worker".to_string(),
        max_context: 16_000,
        supports_streaming: false,
        kind: kind.to_string(),
        metadata: BTreeMap::new(),
    }
}

fn task(id: &str, role: &str, runner_ids: Vec<String>) -> TaskConfig {
    TaskConfig {
        id: id.to_string(),
        role: role.to_string(),
        objective: "Deliver the answer".to_string(),
        context: "Use the supplied context".to_string(),
        boundaries: "Return structured output only".to_string(),
        output_schema: json!({"type": "object"}),
        runner_ids,
        inputs: BTreeMap::new(),
        limits: RunLimits::default(),
        permissions: PermissionMode::Readonly,
        metadata: BTreeMap::new(),
    }
}

#[derive(Debug)]
struct CapturedRequest {
    path: String,
    body: String,
}

fn start_json_server(
    response: Value,
) -> (
    String,
    mpsc::Receiver<CapturedRequest>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test server binds");
    let addr = listener
        .local_addr()
        .expect("test server local addr exists");
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _addr) = listener.accept().expect("test server accepts request");
        let request = read_request(&mut stream);
        tx.send(request).expect("captured request is sent");
        let body = response.to_string();
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(reply.as_bytes())
            .expect("test response is written");
    });
    (format!("http://{addr}"), rx, handle)
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 512];
    loop {
        let read = stream.read(&mut chunk).expect("request chunk is read");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(request) = parse_request(&buffer) {
            return request;
        }
    }
    parse_request(&buffer).expect("request is complete")
}

fn parse_request(buffer: &[u8]) -> Option<CapturedRequest> {
    let text = String::from_utf8_lossy(buffer);
    let header_end = text.find("\r\n\r\n")?;
    let headers = &text[..header_end];
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.strip_prefix("content-length: ")
                .or_else(|| line.strip_prefix("Content-Length: "))
        })
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or_default();
    let body_start = header_end + 4;
    if buffer.len() < body_start + content_length {
        return None;
    }
    let request_line = headers.lines().next()?;
    let path = request_line.split_whitespace().nth(1)?.to_string();
    let body =
        String::from_utf8_lossy(&buffer[body_start..body_start + content_length]).to_string();
    Some(CapturedRequest { path, body })
}

fn unique_temp_path(prefix: &str, extension: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after UNIX epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{nanos}.{extension}"))
}
