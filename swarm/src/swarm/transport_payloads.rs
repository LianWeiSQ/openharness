fn result_from_json(value: Value) -> AgentResult {
    if !value.is_object() {
        return normalize_result_payload(ResultPayload::Text(value.to_string()));
    }
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .map(run_status_from_str)
        .unwrap_or(RunStatus::Completed);
    AgentResult {
        status,
        summary: value
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        evidence: string_list(value.get("evidence")),
        open_questions: string_list(value.get("open_questions")),
        confidence: value
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        artifacts: artifacts_from_value(value.get("artifacts")),
        usage: usage_from_value(value.get("usage")),
        metadata: map_from_value(value.get("metadata")),
    }
}

async fn run_subprocess(
    descriptor: &AgentDescriptor,
    command: &SubprocessCommand,
    spec: &AgentSpec,
    ctx: &RunContext,
) -> AgentResult {
    if command.argv.is_empty() {
        return failed_result(
            "subprocess command argv is required",
            "subprocess_start_error",
            &descriptor.id,
        );
    }
    let mut cmd = Command::new(&command.argv[0]);
    cmd.args(&command.argv[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(cwd) = &command.cwd {
        cmd.current_dir(cwd);
    }
    for (key, value) in &command.env {
        cmd.env(key, value);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            return failed_result(error.to_string(), "subprocess_start_error", &descriptor.id);
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let payload = payload_for_runner(spec, ctx, descriptor);
        let raw = match serde_json::to_vec(&payload) {
            Ok(raw) => raw,
            Err(error) => {
                return failed_result(error.to_string(), "payload_serialize_error", &descriptor.id);
            }
        };
        if let Err(error) = stdin.write_all(&raw).await {
            return failed_result(error.to_string(), "subprocess_stdin_error", &descriptor.id);
        }
    }

    let stdout = child.stdout.take().map(read_child_output);
    let stderr = child.stderr.take().map(read_child_output);
    let wait = child.wait();
    let timeout_seconds = timeout_seconds(spec, command.timeout_seconds);
    let status = if let Some(seconds) = timeout_seconds {
        match timeout(Duration::from_secs_f64(seconds), wait).await {
            Ok(status) => status,
            Err(_elapsed) => {
                let _ = child.kill().await;
                let mut metadata = BTreeMap::new();
                metadata.insert("error_kind".to_string(), json!("subprocess_timeout"));
                metadata.insert("runner_id".to_string(), json!(descriptor.id));
                return AgentResult {
                    status: RunStatus::Failed,
                    summary: format!("Subprocess runner timed out after {seconds} seconds."),
                    evidence: Vec::new(),
                    open_questions: Vec::new(),
                    confidence: 0.0,
                    artifacts: Vec::new(),
                    usage: SwarmUsage::default(),
                    metadata,
                };
            }
        }
    } else {
        wait.await
    };

    let status = match status {
        Ok(status) => status,
        Err(error) => {
            return failed_result(error.to_string(), "subprocess_wait_error", &descriptor.id);
        }
    };
    let stdout_text = read_joined(stdout).await;
    let stderr_text = read_joined(stderr).await;
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        let mut metadata = BTreeMap::new();
        metadata.insert("error_kind".to_string(), json!("subprocess_exit_error"));
        metadata.insert("runner_id".to_string(), json!(descriptor.id));
        metadata.insert("returncode".to_string(), json!(code));
        metadata.insert("stderr".to_string(), json!(stderr_text));
        return AgentResult {
            status: RunStatus::Failed,
            summary: if stderr_text.is_empty() {
                stdout_text
            } else {
                stderr_text
            },
            evidence: Vec::new(),
            open_questions: Vec::new(),
            confidence: 0.0,
            artifacts: Vec::new(),
            usage: SwarmUsage::default(),
            metadata,
        };
    }
    normalize_transport_body(
        descriptor,
        stdout_text.trim(),
        BTreeMap::from([
            ("returncode".to_string(), json!(status.code().unwrap_or(0))),
            ("runner_id".to_string(), json!(descriptor.id)),
        ]),
        "stdout_format",
    )
}

async fn run_http(
    descriptor: &AgentDescriptor,
    request: &HttpRequestConfig,
    spec: &AgentSpec,
    ctx: &RunContext,
    a2a: bool,
) -> AgentResult {
    let timeout = timeout_seconds(spec, request.timeout_seconds).unwrap_or(30.0);
    let client = match reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs_f64(timeout))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return failed_result(error.to_string(), "http_client_error", &descriptor.id);
        }
    };
    let payload = if a2a {
        payload_for_a2a(spec, ctx, &descriptor.id)
    } else {
        payload_for_runner(spec, ctx, descriptor)
    };
    let method = request
        .method
        .parse::<reqwest::Method>()
        .unwrap_or(reqwest::Method::POST);
    let mut builder = client.request(method, &request.url).json(&payload);
    for (key, value) in &request.headers {
        builder = builder.header(key, value);
    }
    let response = match builder.send().await {
        Ok(response) => response,
        Err(error) => {
            let kind = if a2a {
                "a2a_request_error"
            } else {
                "http_request_error"
            };
            return failed_result(error.to_string(), kind, &descriptor.id);
        }
    };
    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            return failed_result(error.to_string(), "http_body_error", &descriptor.id);
        }
    };
    if !status.is_success() {
        let kind = if a2a {
            "a2a_http_status_error"
        } else {
            "http_status_error"
        };
        let mut result = failed_result(body, kind, &descriptor.id);
        result
            .metadata
            .insert("http_status".to_string(), json!(status.as_u16()));
        return result;
    }
    normalize_transport_body(
        descriptor,
        body.trim(),
        BTreeMap::from([
            ("http_status".to_string(), json!(status.as_u16())),
            ("runner_id".to_string(), json!(descriptor.id)),
        ]),
        "response_format",
    )
}

async fn read_child_output(mut output: impl AsyncRead + Unpin) -> String {
    let mut text = String::new();
    match output.read_to_string(&mut text).await {
        Ok(_bytes) => text,
        Err(error) => error.to_string(),
    }
}

async fn read_joined(task: Option<impl Future<Output = String>>) -> String {
    if let Some(task) = task {
        task.await
    } else {
        String::new()
    }
}

fn normalize_transport_body(
    descriptor: &AgentDescriptor,
    body: &str,
    mut metadata: BTreeMap<String, Value>,
    format_key: &str,
) -> AgentResult {
    if body.is_empty() {
        metadata.insert(format_key.to_string(), json!("empty"));
        return AgentResult {
            status: RunStatus::Completed,
            summary: "runner completed without response body.".to_string(),
            evidence: Vec::new(),
            open_questions: Vec::new(),
            confidence: 0.0,
            artifacts: Vec::new(),
            usage: SwarmUsage::default(),
            metadata,
        };
    }
    match serde_json::from_str::<Value>(body) {
        Ok(value) => {
            let mut result = normalize_result_payload(ResultPayload::Json(value));
            result
                .metadata
                .insert("runner_id".to_string(), json!(descriptor.id));
            result
                .metadata
                .insert(format_key.to_string(), json!("json"));
            result.metadata.extend(metadata);
            result
        }
        Err(_error) => {
            metadata.insert(format_key.to_string(), json!("text"));
            AgentResult {
                status: RunStatus::Completed,
                summary: body.to_string(),
                evidence: Vec::new(),
                open_questions: Vec::new(),
                confidence: 0.0,
                artifacts: Vec::new(),
                usage: SwarmUsage::default(),
                metadata,
            }
        }
    }
}

fn payload_for_runner(spec: &AgentSpec, ctx: &RunContext, descriptor: &AgentDescriptor) -> Value {
    json!({
        "schema_version": 1,
        "runner": descriptor,
        "spec": spec,
        "context": ctx,
    })
}

fn payload_for_a2a(spec: &AgentSpec, ctx: &RunContext, runner_id: &str) -> Value {
    let text = format!(
        "Role: {}\nObjective: {}\nContext: {}\nBoundaries: {}\nOutput schema: {}\nInputs: {}",
        spec.role,
        spec.objective,
        spec.context,
        spec.boundaries,
        stable_json(&spec.output_schema),
        stable_json(&json!(spec.inputs)),
    );
    json!({
        "message": {
            "role": "ROLE_USER",
            "parts": [{"text": text}],
            "messageId": format!("{}:{}:{}", ctx.run_id, runner_id, generated_run_id()),
            "contextId": ctx.run_id,
        },
        "configuration": {
            "acceptedOutputModes": ["text/plain"],
            "metadata": {
                "swarm_run_id": ctx.run_id,
                "swarm_runner_id": runner_id,
                "swarm_parent_span_id": ctx.parent_span_id,
            },
        },
    })
}
