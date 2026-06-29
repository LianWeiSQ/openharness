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
