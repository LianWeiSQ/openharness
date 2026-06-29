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
