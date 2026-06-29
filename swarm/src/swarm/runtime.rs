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
