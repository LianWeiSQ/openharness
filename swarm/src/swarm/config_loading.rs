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
