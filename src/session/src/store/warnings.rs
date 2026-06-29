#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RuntimeWarningConfig {
    pub enabled: bool,
    pub context_usage_ratio: Option<f64>,
    pub context_critical_ratio: Option<f64>,
    pub max_step_input_tokens: Option<u64>,
    pub max_step_output_tokens: Option<u64>,
    pub max_step_total_tokens: Option<u64>,
    pub max_step_cost: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeWarningRecord {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub metrics: BTreeMap<String, Value>,
}

impl RuntimeWarningRecord {
    #[must_use]
    pub fn to_event(&self) -> Value {
        json!({
            "type": "runtime-warning",
            "severity": self.severity,
            "code": self.code,
            "message": self.message,
            "metrics": self.metrics,
            "display": {
                "kind": "runtime_warning",
                "severity": self.severity,
                "title": warning_title(&self.code),
                "body": self.message,
                "metrics": display_metrics(&self.code, &self.metrics),
            },
        })
    }
}

#[must_use]
pub fn step_usage_warnings(
    config: &RuntimeWarningConfig,
    usage: &Usage,
    step_index: u64,
) -> Vec<RuntimeWarningRecord> {
    if !config.enabled {
        return Vec::new();
    }
    let total_tokens = usage.input_tokens + usage.output_tokens;
    let base_metrics = BTreeMap::from([
        ("step_index".to_string(), json!(step_index)),
        ("input_tokens".to_string(), json!(usage.input_tokens)),
        ("output_tokens".to_string(), json!(usage.output_tokens)),
        ("total_tokens".to_string(), json!(total_tokens)),
        ("cost".to_string(), json!(usage.cost)),
    ]);
    let mut warnings = Vec::new();
    if config
        .max_step_input_tokens
        .is_some_and(|threshold| usage.input_tokens > threshold)
    {
        let threshold = config.max_step_input_tokens.unwrap_or_default();
        warnings.push(RuntimeWarningRecord {
            code: "step_input_tokens_exceeded".to_string(),
            severity: "warning".to_string(),
            message: format!(
                "Step input tokens exceeded budget: {} > {threshold}.",
                usage.input_tokens
            ),
            metrics: metrics_with_threshold(&base_metrics, threshold),
        });
    }
    if config
        .max_step_output_tokens
        .is_some_and(|threshold| usage.output_tokens > threshold)
    {
        let threshold = config.max_step_output_tokens.unwrap_or_default();
        warnings.push(RuntimeWarningRecord {
            code: "step_output_tokens_exceeded".to_string(),
            severity: "warning".to_string(),
            message: format!(
                "Step output tokens exceeded budget: {} > {threshold}.",
                usage.output_tokens
            ),
            metrics: metrics_with_threshold(&base_metrics, threshold),
        });
    }
    if config
        .max_step_total_tokens
        .is_some_and(|threshold| total_tokens > threshold)
    {
        let threshold = config.max_step_total_tokens.unwrap_or_default();
        warnings.push(RuntimeWarningRecord {
            code: "step_total_tokens_exceeded".to_string(),
            severity: "warning".to_string(),
            message: format!("Step total tokens exceeded budget: {total_tokens} > {threshold}."),
            metrics: metrics_with_threshold(&base_metrics, threshold),
        });
    }
    if config
        .max_step_cost
        .is_some_and(|threshold| usage.cost > threshold)
    {
        let threshold = config.max_step_cost.unwrap_or_default();
        let mut metrics = base_metrics.clone();
        metrics.insert("threshold".to_string(), json!(threshold));
        warnings.push(RuntimeWarningRecord {
            code: "step_cost_exceeded".to_string(),
            severity: "warning".to_string(),
            message: format!(
                "Step cost exceeded budget: {:.6} > {threshold:.6}.",
                usage.cost
            ),
            metrics,
        });
    }
    warnings
}

#[must_use]
pub fn format_runtime_warning_event(event: &Value) -> Option<String> {
    if event.get("type").and_then(Value::as_str) != Some("runtime-warning") {
        return None;
    }
    let display = event.get("display").and_then(Value::as_object);
    let severity = display
        .and_then(|items| items.get("severity"))
        .or_else(|| event.get("severity"))
        .and_then(Value::as_str)
        .unwrap_or("warning")
        .to_uppercase();
    let title = display
        .and_then(|items| items.get("title"))
        .or_else(|| event.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("Runtime warning");
    let body = display
        .and_then(|items| items.get("body"))
        .or_else(|| event.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let metric_text = display
        .and_then(|items| items.get("metrics"))
        .and_then(Value::as_object)
        .map(format_display_metrics)
        .unwrap_or_default();
    let suffix = if metric_text.is_empty() {
        String::new()
    } else {
        format!(" ({metric_text})")
    };
    Some(format!("[{severity}] {title}: {body}{suffix}"))
}

#[must_use]
pub fn sanitize_trace_value(value: Value) -> Value {
    sanitize_value(value, DEFAULT_FIELD_PREVIEW_CHARS)
}

#[must_use]
pub fn sanitize_observation_value(value: Value) -> Value {
    sanitize_value(value, DEFAULT_FIELD_PREVIEW_CHARS)
}

#[must_use]
pub fn input_preview(value: Value, max_chars: usize) -> String {
    truncate_text(
        &stable_json_dumps(&sanitize_value(value, max_chars)),
        max_chars,
    )
}

#[must_use]
pub fn output_stats(output: &str) -> BTreeMap<String, u64> {
    BTreeMap::from([
        ("output_bytes".to_string(), output.len() as u64),
        ("output_lines".to_string(), output.lines().count() as u64),
    ])
}
