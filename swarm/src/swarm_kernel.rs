//! Agent-agnostic swarm kernel crate for the Rust rewrite.

use std::{
    collections::BTreeMap,
    future::Future,
    path::Path,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use openagent_protocol::{
    AgentDescriptor, AgentResult, AgentSpec, ArtifactRef, FanoutBudget, PermissionMode, RunContext,
    RunLimits, RunStatus, SwarmUsage,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

pub type SwarmError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type SwarmResult<T> = Result<T, SwarmError>;
pub type FunctionFuture = Pin<Box<dyn Future<Output = ResultPayload> + Send>>;
pub type FunctionHandler = Arc<dyn Fn(AgentSpec, RunContext) -> FunctionFuture + Send + Sync>;

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn command_name() -> &'static str {
    "openagent-swarm"
}

#[must_use]
pub fn protocol_crate_name() -> &'static str {
    openagent_protocol::crate_name()
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentEvent {
    pub event_type: String,
    pub run_id: String,
    pub runner_id: String,
    pub message: String,
    pub metadata: BTreeMap<String, Value>,
}

impl AgentEvent {
    #[must_use]
    pub fn new(
        event_type: impl Into<String>,
        run_id: impl Into<String>,
        runner_id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            run_id: run_id.into(),
            runner_id: runner_id.into(),
            message: message.into(),
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunnerRunResult {
    pub events: Vec<AgentEvent>,
    pub result: AgentResult,
}

#[async_trait]
pub trait AgentRunner: Send + Sync {
    fn descriptor(&self) -> &AgentDescriptor;

    async fn start(&self, spec: AgentSpec, ctx: RunContext) -> RunnerRunResult;
}

#[derive(Clone, Default)]
pub struct RunnerRegistry {
    runners: BTreeMap<String, Arc<dyn AgentRunner>>,
}

impl RunnerRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<R>(&mut self, runner: R)
    where
        R: AgentRunner + 'static,
    {
        let id = runner.descriptor().id.clone();
        self.runners.insert(id, Arc::new(runner));
    }

    pub fn register_arc(&mut self, runner: Arc<dyn AgentRunner>) {
        let id = runner.descriptor().id.clone();
        self.runners.insert(id, runner);
    }

    #[must_use]
    pub fn require(&self, runner_id: &str) -> Option<Arc<dyn AgentRunner>> {
        self.runners.get(runner_id).cloned()
    }

    #[must_use]
    pub fn matching_role(&self, role: &str) -> Vec<Arc<dyn AgentRunner>> {
        self.runners
            .values()
            .filter(|runner| supports_role(runner.descriptor(), role))
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.runners.keys().cloned().collect()
    }
}

pub struct FunctionRunner {
    descriptor: AgentDescriptor,
    handler: FunctionHandler,
}

impl FunctionRunner {
    #[must_use]
    pub fn new(descriptor: AgentDescriptor, handler: FunctionHandler) -> Self {
        Self {
            descriptor,
            handler,
        }
    }
}

#[async_trait]
impl AgentRunner for FunctionRunner {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    async fn start(&self, spec: AgentSpec, ctx: RunContext) -> RunnerRunResult {
        let started = started_event(&ctx, &self.descriptor, &spec, "function");
        let result = match validate_spec(&spec) {
            Ok(()) => normalize_result_payload((self.handler)(spec, ctx.clone()).await),
            Err(error) => failed_result(error, "agent_spec_validation_error", &self.descriptor.id),
        };
        let finished = finished_event(&ctx, &self.descriptor, &result, "function");
        RunnerRunResult {
            events: vec![started, finished],
            result,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SubprocessCommand {
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_seconds: Option<f64>,
}

pub struct SubprocessRunner {
    descriptor: AgentDescriptor,
    command: SubprocessCommand,
}

impl SubprocessRunner {
    #[must_use]
    pub fn new(descriptor: AgentDescriptor, command: SubprocessCommand) -> Self {
        Self {
            descriptor,
            command,
        }
    }
}

#[async_trait]
impl AgentRunner for SubprocessRunner {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    async fn start(&self, spec: AgentSpec, ctx: RunContext) -> RunnerRunResult {
        let started = started_event(&ctx, &self.descriptor, &spec, "subprocess");
        let result = match validate_spec(&spec) {
            Ok(()) => run_subprocess(&self.descriptor, &self.command, &spec, &ctx).await,
            Err(error) => failed_result(error, "agent_spec_validation_error", &self.descriptor.id),
        };
        let finished = finished_event(&ctx, &self.descriptor, &result, "subprocess");
        RunnerRunResult {
            events: vec![started, finished],
            result,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct HttpRequestConfig {
    pub url: String,
    #[serde(default = "default_post")]
    pub method: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_seconds: Option<f64>,
}

pub struct HttpRunner {
    descriptor: AgentDescriptor,
    request: HttpRequestConfig,
}

impl HttpRunner {
    #[must_use]
    pub fn new(descriptor: AgentDescriptor, request: HttpRequestConfig) -> Self {
        Self {
            descriptor,
            request,
        }
    }
}

#[async_trait]
impl AgentRunner for HttpRunner {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    async fn start(&self, spec: AgentSpec, ctx: RunContext) -> RunnerRunResult {
        let started = started_event(&ctx, &self.descriptor, &spec, "http");
        let result = match validate_spec(&spec) {
            Ok(()) => run_http(&self.descriptor, &self.request, &spec, &ctx, false).await,
            Err(error) => failed_result(error, "agent_spec_validation_error", &self.descriptor.id),
        };
        let finished = finished_event(&ctx, &self.descriptor, &result, "http");
        RunnerRunResult {
            events: vec![started, finished],
            result,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct A2ARequestConfig {
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_seconds: Option<f64>,
}

pub struct A2ARunner {
    descriptor: AgentDescriptor,
    request: A2ARequestConfig,
}

impl A2ARunner {
    #[must_use]
    pub fn new(descriptor: AgentDescriptor, request: A2ARequestConfig) -> Self {
        Self {
            descriptor,
            request,
        }
    }
}

#[async_trait]
impl AgentRunner for A2ARunner {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    async fn start(&self, spec: AgentSpec, ctx: RunContext) -> RunnerRunResult {
        let started = started_event(&ctx, &self.descriptor, &spec, "a2a");
        let request = HttpRequestConfig {
            url: message_send_url(&self.request.url),
            method: "POST".to_string(),
            headers: self.request.headers.clone(),
            timeout_seconds: self.request.timeout_seconds,
        };
        let result = match validate_spec(&spec) {
            Ok(()) => run_http(&self.descriptor, &request, &spec, &ctx, true).await,
            Err(error) => failed_result(error, "agent_spec_validation_error", &self.descriptor.id),
        };
        let finished = finished_event(&ctx, &self.descriptor, &result, "a2a");
        RunnerRunResult {
            events: vec![started, finished],
            result,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SwarmConfig {
    #[serde(default)]
    pub fanout_budget: FanoutBudget,
    #[serde(default, deserialize_with = "deserialize_runner_configs")]
    pub runners: Vec<RunnerConfig>,
    #[serde(default, deserialize_with = "deserialize_task_configs")]
    pub tasks: Vec<TaskConfig>,
}

impl SwarmConfig {
    #[must_use]
    pub fn task(&self, task_id: &str) -> Option<TaskConfig> {
        self.tasks.iter().find(|task| task.id == task_id).cloned()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RunnerConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default = "default_function_kind")]
    pub kind: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tool_groups: Vec<String>,
    #[serde(default = "default_model_tier")]
    pub model_tier: String,
    #[serde(default)]
    pub max_context: u64,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl RunnerConfig {
    #[must_use]
    pub fn to_descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            id: self.id.clone(),
            roles: self.roles.clone(),
            tool_groups: self.tool_groups.clone(),
            model_tier: self.model_tier.clone(),
            max_context: self.max_context,
            supports_streaming: self.supports_streaming,
            kind: self.kind.clone(),
            metadata: self.metadata.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TaskConfig {
    #[serde(default)]
    pub id: String,
    pub role: String,
    pub objective: String,
    pub context: String,
    pub boundaries: String,
    pub output_schema: Value,
    #[serde(default)]
    pub runner_ids: Vec<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub limits: RunLimits,
    #[serde(default = "default_permission_mode")]
    pub permissions: PermissionMode,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResultPayload {
    Text(String),
    Json(Value),
    Result(AgentResult),
}

impl From<&str> for ResultPayload {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<String> for ResultPayload {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<Value> for ResultPayload {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}

impl From<AgentResult> for ResultPayload {
    fn from(value: AgentResult) -> Self {
        Self::Result(value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwarmRunResult {
    pub task_id: String,
    pub status: String,
    pub summary: String,
    pub results: BTreeMap<String, AgentResult>,
    pub usage: SwarmUsage,
    pub warnings: Vec<String>,
    pub events: Vec<AgentEvent>,
}

pub struct SwarmRuntime {
    registry: RunnerRegistry,
    fanout_budget: FanoutBudget,
}

impl SwarmRuntime {
    #[must_use]
    pub fn new(registry: RunnerRegistry, fanout_budget: FanoutBudget) -> Self {
        Self {
            registry,
            fanout_budget: fanout_budget.normalized(),
        }
    }

    pub async fn run_task(&self, task: &TaskConfig, run_id: Option<String>) -> SwarmRunResult {
        let run_id = run_id.unwrap_or_else(generated_run_id);
        let mut warnings = Vec::new();
        let mut runner_ids = match self.resolve_runners(task) {
            Ok(ids) => ids,
            Err(error) => {
                return SwarmRunResult {
                    task_id: task.id.clone(),
                    status: "failed".to_string(),
                    summary: error,
                    results: BTreeMap::new(),
                    usage: SwarmUsage::default(),
                    warnings,
                    events: Vec::new(),
                };
            }
        };

        if runner_ids.len() > self.fanout_budget.max_total_workers as usize {
            warnings.push(format!(
                "runner count {} exceeds max_total_workers {}; truncating",
                runner_ids.len(),
                self.fanout_budget.max_total_workers
            ));
            runner_ids.truncate(self.fanout_budget.max_total_workers as usize);
        }

        let mut results = BTreeMap::new();
        let mut events = Vec::new();
        for runner_id in &runner_ids {
            let Some(runner) = self.registry.require(runner_id) else {
                results.insert(
                    runner_id.clone(),
                    failed_result(
                        format!("unknown runner: {runner_id}"),
                        "runner_dispatch_error",
                        runner_id,
                    ),
                );
                continue;
            };
            let role = if supports_role(runner.descriptor(), &task.role) {
                task.role.clone()
            } else {
                runner
                    .descriptor()
                    .roles
                    .first()
                    .cloned()
                    .unwrap_or_else(|| task.role.clone())
            };
            let spec = AgentSpec {
                role,
                objective: task.objective.clone(),
                context: task.context.clone(),
                boundaries: task.boundaries.clone(),
                output_schema: task.output_schema.clone(),
                inputs: task.inputs.clone(),
                limits: task.limits.clone(),
                permissions: task.permissions.clone(),
                metadata: {
                    let mut metadata = task.metadata.clone();
                    metadata.insert("task_id".to_string(), Value::String(task.id.clone()));
                    metadata.insert("runner_id".to_string(), Value::String(runner_id.clone()));
                    metadata
                },
            };
            let ctx = RunContext {
                run_id: run_id.clone(),
                parent_span_id: None,
                budget: self.fanout_budget.clone(),
                cancellation: None,
                metadata: BTreeMap::from([
                    ("task_id".to_string(), Value::String(task.id.clone())),
                    ("runner_id".to_string(), Value::String(runner_id.clone())),
                ]),
            };
            let outcome = runner.start(spec, ctx).await;
            events.extend(outcome.events);
            results.insert(runner_id.clone(), outcome.result);
        }

        let usage = aggregate_usage(results.values());
        warnings.extend(budget_warnings(&usage, &self.fanout_budget));
        let status = aggregate_status(&results);
        let summary = aggregate_summary(&results);
        SwarmRunResult {
            task_id: task.id.clone(),
            status,
            summary,
            results,
            usage,
            warnings,
            events,
        }
    }

    fn resolve_runners(&self, task: &TaskConfig) -> Result<Vec<String>, String> {
        if !task.runner_ids.is_empty() {
            return Ok(task.runner_ids.clone());
        }
        let ids = self
            .registry
            .matching_role(&task.role)
            .iter()
            .map(|runner| runner.descriptor().id.clone())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            Err(format!("no runner matches role \"{}\"", task.role))
        } else {
            Ok(ids)
        }
    }
}

pub fn load_swarm_config_from_str(raw: &str) -> SwarmResult<SwarmConfig> {
    let config = serde_yaml::from_str::<SwarmConfig>(raw)?;
    Ok(config)
}

pub fn load_swarm_config(path: impl AsRef<Path>) -> SwarmResult<SwarmConfig> {
    let raw = std::fs::read_to_string(path)?;
    load_swarm_config_from_str(&raw)
}

pub fn build_transport_registry(config: &SwarmConfig) -> SwarmResult<RunnerRegistry> {
    let mut registry = RunnerRegistry::new();
    for runner in &config.runners {
        match runner.kind.as_str() {
            "subprocess" => registry.register(SubprocessRunner::new(
                runner.to_descriptor(),
                command_from_metadata(&runner.metadata)?,
            )),
            "http" => registry.register(HttpRunner::new(
                runner.to_descriptor(),
                http_request_from_metadata(&runner.metadata)?,
            )),
            "a2a" => registry.register(A2ARunner::new(
                runner.to_descriptor(),
                a2a_request_from_metadata(&runner.metadata)?,
            )),
            "function" => {}
            other => {
                return Err(format!("unsupported runner kind for Rust CLI: {other}").into());
            }
        }
    }
    Ok(registry)
}

#[must_use]
pub fn swarm_run_result_to_json(result: &SwarmRunResult, run_id: &str) -> Value {
    let runner_results = result
        .results
        .iter()
        .map(|(runner_id, result)| (runner_id.clone(), json!(result)))
        .collect::<serde_json::Map<_, _>>();
    json!({
        "run_id": run_id,
        "task_id": result.task_id,
        "status": result.status,
        "summary": result.summary,
        "results": runner_results,
        "usage": result.usage,
        "warnings": result.warnings,
        "trace_event_count": result.events.len(),
    })
}

pub fn normalize_result_payload(value: ResultPayload) -> AgentResult {
    match value {
        ResultPayload::Result(result) => result,
        ResultPayload::Text(summary) => AgentResult {
            status: RunStatus::Completed,
            summary,
            evidence: Vec::new(),
            open_questions: Vec::new(),
            confidence: 0.0,
            artifacts: Vec::new(),
            usage: SwarmUsage::default(),
            metadata: BTreeMap::new(),
        },
        ResultPayload::Json(value) => result_from_json(value),
    }
}

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

fn validate_spec(spec: &AgentSpec) -> Result<(), String> {
    let mut missing = Vec::new();
    if spec.role.trim().is_empty() {
        missing.push("role");
    }
    if spec.objective.trim().is_empty() {
        missing.push("objective");
    }
    if spec.context.trim().is_empty() {
        missing.push("context");
    }
    if spec.boundaries.trim().is_empty() {
        missing.push("boundaries");
    }
    if !spec.output_schema.is_object()
        || spec
            .output_schema
            .as_object()
            .map(serde_json::Map::is_empty)
            .unwrap_or(true)
    {
        missing.push("output_schema");
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "AgentSpec is missing required contract fields: {}",
            missing.join(", ")
        ))
    }
}

fn failed_result(
    summary: impl Into<String>,
    error_kind: impl Into<String>,
    runner_id: impl Into<String>,
) -> AgentResult {
    AgentResult {
        status: RunStatus::Failed,
        summary: summary.into(),
        evidence: Vec::new(),
        open_questions: Vec::new(),
        confidence: 0.0,
        artifacts: Vec::new(),
        usage: SwarmUsage::default(),
        metadata: BTreeMap::from([
            ("error_kind".to_string(), json!(error_kind.into())),
            ("runner_id".to_string(), json!(runner_id.into())),
        ]),
    }
}

fn started_event(
    ctx: &RunContext,
    descriptor: &AgentDescriptor,
    spec: &AgentSpec,
    transport: &str,
) -> AgentEvent {
    let mut event = AgentEvent::new(
        "runner.started",
        &ctx.run_id,
        &descriptor.id,
        format!("Started {}", descriptor.id),
    );
    event.metadata.insert("role".to_string(), json!(spec.role));
    event
        .metadata
        .insert("kind".to_string(), json!(descriptor.kind));
    event
        .metadata
        .insert("transport".to_string(), json!(transport));
    event
}

fn finished_event(
    ctx: &RunContext,
    descriptor: &AgentDescriptor,
    result: &AgentResult,
    transport: &str,
) -> AgentEvent {
    let mut event = AgentEvent::new(
        "runner.finished",
        &ctx.run_id,
        &descriptor.id,
        result.summary.clone(),
    );
    event
        .metadata
        .insert("status".to_string(), json!(status_str(result)));
    event
        .metadata
        .insert("confidence".to_string(), json!(result.confidence));
    event
        .metadata
        .insert("transport".to_string(), json!(transport));
    event
}

fn supports_role(descriptor: &AgentDescriptor, role: &str) -> bool {
    descriptor
        .roles
        .iter()
        .any(|item| item == role || item == "*")
}

fn aggregate_status(results: &BTreeMap<String, AgentResult>) -> String {
    if results.is_empty() {
        return "failed".to_string();
    }
    let statuses = results.values().map(status_str).collect::<Vec<_>>();
    if statuses.iter().all(|status| *status == "completed") {
        "completed".to_string()
    } else if statuses
        .iter()
        .any(|status| *status == "completed" || *status == "partial")
    {
        "partial".to_string()
    } else if statuses.contains(&"cancelled") {
        "cancelled".to_string()
    } else {
        "failed".to_string()
    }
}

fn aggregate_summary(results: &BTreeMap<String, AgentResult>) -> String {
    results
        .iter()
        .map(|(runner_id, result)| {
            format!("[{runner_id}] {}: {}", status_str(result), result.summary)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn aggregate_usage<'a>(results: impl Iterator<Item = &'a AgentResult>) -> SwarmUsage {
    results.fold(SwarmUsage::default(), |mut usage, result| {
        usage.input_tokens += result.usage.input_tokens;
        usage.output_tokens += result.usage.output_tokens;
        usage.cost += result.usage.cost;
        usage.steps += result.usage.steps;
        usage.latency_ms += result.usage.latency_ms;
        usage
    })
}

fn budget_warnings(usage: &SwarmUsage, budget: &FanoutBudget) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some(max_tokens) = budget.max_total_tokens
        && usage.input_tokens + usage.output_tokens > max_tokens
    {
        warnings.push(format!(
            "usage tokens {} exceeds max_total_tokens {max_tokens}",
            usage.input_tokens + usage.output_tokens
        ));
    }
    if let Some(max_cost) = budget.max_total_cost
        && usage.cost > max_cost
    {
        warnings.push(format!(
            "usage cost {:.6} exceeds max_total_cost {:.6}",
            usage.cost, max_cost
        ));
    }
    warnings
}

fn timeout_seconds(spec: &AgentSpec, fallback: Option<f64>) -> Option<f64> {
    spec.limits
        .timeout_seconds
        .as_ref()
        .and_then(Value::as_f64)
        .or(fallback)
}

fn run_status_from_str(value: &str) -> RunStatus {
    match value {
        "partial" => RunStatus::Partial,
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        _ => RunStatus::Completed,
    }
}

fn status_str(result: &AgentResult) -> &'static str {
    match result.status {
        RunStatus::Completed => "completed",
        RunStatus::Partial => "partial",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(item)) => vec![item.clone()],
        _ => Vec::new(),
    }
}

fn artifacts_from_value(value: Option<&Value>) -> Vec<ArtifactRef> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| serde_json::from_value::<ArtifactRef>(item.clone()).ok())
            .collect(),
        _ => Vec::new(),
    }
}

fn usage_from_value(value: Option<&Value>) -> SwarmUsage {
    match value {
        Some(value) => serde_json::from_value::<SwarmUsage>(value.clone()).unwrap_or_default(),
        None => SwarmUsage::default(),
    }
}

fn map_from_value(value: Option<&Value>) -> BTreeMap<String, Value> {
    match value {
        Some(Value::Object(items)) => items.clone().into_iter().collect(),
        _ => BTreeMap::new(),
    }
}

fn command_from_metadata(metadata: &BTreeMap<String, Value>) -> SwarmResult<SubprocessCommand> {
    let raw = metadata
        .get("command")
        .or_else(|| metadata.get("argv"))
        .ok_or_else(|| "subprocess runner metadata.command is required".to_string())?;
    let argv = match raw {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        Value::String(command) => command
            .split_whitespace()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let env = metadata
        .get("env")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    Ok(SubprocessCommand {
        argv,
        cwd: metadata
            .get("cwd")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        env,
        timeout_seconds: metadata.get("timeout_seconds").and_then(Value::as_f64),
    })
}

fn http_request_from_metadata(
    metadata: &BTreeMap<String, Value>,
) -> SwarmResult<HttpRequestConfig> {
    let url = metadata
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "http runner metadata.url is required".to_string())?
        .to_string();
    Ok(HttpRequestConfig {
        url,
        method: metadata
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("POST")
            .to_uppercase(),
        headers: string_map(metadata.get("headers")),
        timeout_seconds: metadata.get("timeout_seconds").and_then(Value::as_f64),
    })
}

fn a2a_request_from_metadata(metadata: &BTreeMap<String, Value>) -> SwarmResult<A2ARequestConfig> {
    let url = metadata
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "a2a runner metadata.url is required".to_string())?
        .to_string();
    Ok(A2ARequestConfig {
        url,
        headers: string_map(metadata.get("headers")),
        timeout_seconds: metadata.get("timeout_seconds").and_then(Value::as_f64),
    })
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default()
}

fn message_send_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with("/message/send") || trimmed.ends_with("/message/stream") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/message/send")
    }
}

fn stable_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
}

fn generated_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("swarm_{nanos}")
}

fn default_post() -> String {
    "POST".to_string()
}

fn default_function_kind() -> String {
    "function".to_string()
}

fn default_model_tier() -> String {
    "worker".to_string()
}

fn default_permission_mode() -> PermissionMode {
    PermissionMode::Readonly
}

fn deserialize_runner_configs<'de, D>(deserializer: D) -> Result<Vec<RunnerConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| {
                serde_json::from_value::<RunnerConfig>(item).map_err(serde::de::Error::custom)
            })
            .collect(),
        Value::Object(items) => items
            .into_iter()
            .map(|(id, value)| {
                let mut config = serde_json::from_value::<RunnerConfig>(value)
                    .map_err(serde::de::Error::custom)?;
                config.id = id;
                Ok(config)
            })
            .collect(),
        _ => Ok(Vec::new()),
    }
}

fn deserialize_task_configs<'de, D>(deserializer: D) -> Result<Vec<TaskConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| {
                serde_json::from_value::<TaskConfig>(item).map_err(serde::de::Error::custom)
            })
            .collect(),
        Value::Object(items) => items
            .into_iter()
            .map(|(id, value)| {
                let mut config = serde_json::from_value::<TaskConfig>(value)
                    .map_err(serde::de::Error::custom)?;
                config.id = id;
                Ok(config)
            })
            .collect(),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_command_boundary() {
        assert_eq!(crate_name(), "openagent-swarm");
        assert_eq!(command_name(), "openagent-swarm");
        assert_eq!(protocol_crate_name(), "openagent-protocol");
    }
}
