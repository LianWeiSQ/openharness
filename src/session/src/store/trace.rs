#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TraceConfig {
    pub enabled: bool,
    pub root_dir: String,
    pub keep_events: bool,
    pub max_events: u64,
    pub write_summary: bool,
    pub exporters: BTreeMap<String, Value>,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            root_dir: DEFAULT_TRACE_ROOT.to_string(),
            keep_events: true,
            max_events: DEFAULT_TRACE_MAX_EVENTS,
            write_summary: true,
            exporters: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunRecord {
    pub run_id: String,
    pub trace_id: String,
    pub session_id: String,
    pub agent_name: String,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
    pub workspace: Option<String>,
    pub started_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TraceEvent {
    pub seq: u64,
    pub event: String,
    pub timestamp_ms: u64,
    pub run_id: String,
    pub trace_id: String,
    pub session_id: String,
    pub event_id: Option<String>,
    pub kind: String,
    pub status: String,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub duration_ms: Option<u64>,
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default)]
pub struct TraceEventOptions {
    pub event_id: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub duration_ms: Option<u64>,
    pub timestamp_ms: Option<u64>,
    pub attributes: BTreeMap<String, Value>,
}

pub struct AgentTraceRecorder<'a> {
    pub run: RunRecord,
    pub config: TraceConfig,
    pub base_dir: PathBuf,
    session_metadata: Option<&'a mut BTreeMap<String, Value>>,
    seq: u64,
    events: Vec<Value>,
    summary: Map<String, Value>,
    closed: bool,
}

impl<'a> AgentTraceRecorder<'a> {
    pub fn new(
        run: RunRecord,
        config: Option<TraceConfig>,
        base_dir: impl Into<PathBuf>,
        session_metadata: Option<&'a mut BTreeMap<String, Value>>,
    ) -> SessionResult<Self> {
        let mut recorder = Self {
            summary: Map::new(),
            run,
            config: config.unwrap_or_default(),
            base_dir: base_dir.into(),
            session_metadata,
            seq: 0,
            events: Vec::new(),
            closed: false,
        };
        recorder.summary = recorder.empty_summary();
        if recorder.config.enabled {
            fs::create_dir_all(recorder.run_dir())?;
            fs::create_dir_all(recorder.artifacts_dir())?;
            recorder.bind_metadata()?;
            recorder.write_process_note("Trace recorder initialized.")?;
            recorder.write_summary()?;
        }
        Ok(recorder)
    }

    #[must_use]
    pub fn root_dir(&self) -> PathBuf {
        let root = PathBuf::from(&self.config.root_dir);
        if root.is_absolute() {
            root
        } else {
            self.base_dir.join(root)
        }
    }

    #[must_use]
    pub fn run_dir(&self) -> PathBuf {
        self.root_dir().join(&self.run.run_id)
    }

    #[must_use]
    pub fn trace_path(&self) -> PathBuf {
        self.run_dir().join("trace.jsonl")
    }

    #[must_use]
    pub fn summary_path(&self) -> PathBuf {
        self.run_dir().join("summary.json")
    }

    #[must_use]
    pub fn process_path(&self) -> PathBuf {
        self.run_dir().join("process.md")
    }

    #[must_use]
    pub fn artifacts_dir(&self) -> PathBuf {
        self.run_dir().join("artifacts")
    }

    pub fn record_event(
        &mut self,
        event: &str,
        options: TraceEventOptions,
    ) -> SessionResult<Option<TraceEvent>> {
        if !self.config.enabled {
            return Ok(None);
        }
        self.seq += 1;
        let trace_event = TraceEvent {
            seq: self.seq,
            event: event.to_string(),
            timestamp_ms: options.timestamp_ms.unwrap_or_else(now_ms),
            run_id: self.run.run_id.clone(),
            trace_id: self.run.trace_id.clone(),
            session_id: self.run.session_id.clone(),
            event_id: options.event_id,
            kind: options.kind.unwrap_or_else(|| "event".to_string()),
            status: if options.status.as_deref() == Some("error") {
                "error"
            } else {
                "ok"
            }
            .to_string(),
            span_id: options.span_id,
            parent_span_id: options.parent_span_id,
            duration_ms: options.duration_ms,
            attributes: sanitize_value_map(options.attributes, DEFAULT_FIELD_PREVIEW_CHARS),
        };
        append_jsonl(&self.trace_path(), &trace_event)?;
        let event_value = serde_json::to_value(&trace_event)?;
        if self.config.keep_events {
            self.events.push(event_value.clone());
            let max_events = self.config.max_events.max(1) as usize;
            if self.events.len() > max_events {
                self.events = self.events[self.events.len() - max_events..].to_vec();
            }
        }
        self.update_summary(&event_value);
        if self.config.write_summary {
            self.write_summary()?;
        }
        if matches!(event, "run.finished" | "run.failed") {
            self.write_process_note(&format!(
                "Run {} after {} trace events.",
                if event == "run.failed" {
                    "failed"
                } else {
                    "completed"
                },
                self.summary
                    .get("event_count")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
            ))?;
            self.close()?;
        }
        Ok(Some(trace_event))
    }

    pub fn finish_run(
        &mut self,
        status: &str,
        attributes: BTreeMap<String, Value>,
    ) -> SessionResult<Option<TraceEvent>> {
        let mut attrs = attributes;
        attrs
            .entry("status".to_string())
            .or_insert_with(|| json!(status));
        self.record_event(
            "run.finished",
            TraceEventOptions {
                kind: Some("run".to_string()),
                attributes: attrs,
                ..TraceEventOptions::default()
            },
        )
    }

    #[must_use]
    pub fn summary(&self) -> Value {
        Value::Object(self.summary.clone())
    }

    pub fn close(&mut self) -> SessionResult<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.sync_exporter_metadata()
    }

    fn bind_metadata(&mut self) -> SessionResult<()> {
        let payload = json!({
            "run_id": self.run.run_id,
            "trace_id": self.run.trace_id,
            "run_dir": self.run_dir().to_string_lossy(),
            "trace_path": self.trace_path().to_string_lossy(),
            "summary_path": self.summary_path().to_string_lossy(),
            "process_path": self.process_path().to_string_lossy(),
            "exporters": {"enabled": [], "diagnostics": []},
        });
        if let Some(metadata) = self.session_metadata.as_mut() {
            (**metadata).insert(TRACE_METADATA_KEY.to_string(), payload);
        }
        Ok(())
    }

    fn sync_exporter_metadata(&mut self) -> SessionResult<()> {
        if let Some(metadata) = self.session_metadata.as_mut()
            && let Some(Value::Object(root)) = (**metadata).get_mut(TRACE_METADATA_KEY)
        {
            root.insert(
                "exporters".to_string(),
                json!({"enabled": [], "diagnostics": []}),
            );
        }
        Ok(())
    }

    fn empty_summary(&self) -> Map<String, Value> {
        Map::from_iter([
            ("run_id".to_string(), json!(self.run.run_id)),
            ("trace_id".to_string(), json!(self.run.trace_id)),
            ("session_id".to_string(), json!(self.run.session_id)),
            ("agent_name".to_string(), json!(self.run.agent_name)),
            ("model_id".to_string(), json!(self.run.model_id)),
            ("provider_id".to_string(), json!(self.run.provider_id)),
            ("workspace".to_string(), json!(self.run.workspace)),
            ("status".to_string(), json!("running")),
            ("started_at_ms".to_string(), json!(self.run.started_at_ms)),
            ("ended_at_ms".to_string(), Value::Null),
            ("duration_ms".to_string(), Value::Null),
            ("event_count".to_string(), json!(0)),
            ("step_count".to_string(), json!(0)),
            ("model_call_count".to_string(), json!(0)),
            ("tool_call_count".to_string(), json!(0)),
            ("mcp_call_count".to_string(), json!(0)),
            ("skill_call_count".to_string(), json!(0)),
            ("local_tool_call_count".to_string(), json!(0)),
            ("artifact_count".to_string(), json!(0)),
            ("error_count".to_string(), json!(0)),
            ("runtime_warning_count".to_string(), json!(0)),
            ("total_latency_ms".to_string(), json!(0)),
            ("total_input_tokens".to_string(), json!(0)),
            ("total_output_tokens".to_string(), json!(0)),
            ("total_reasoning_tokens".to_string(), json!(0)),
            ("total_cache_read_tokens".to_string(), json!(0)),
            ("total_cache_write_tokens".to_string(), json!(0)),
            ("total_cost".to_string(), json!(0.0)),
            ("errors".to_string(), json!([])),
            (
                "paths".to_string(),
                json!({
                    "run_dir": self.run_dir().to_string_lossy(),
                    "trace": self.trace_path().to_string_lossy(),
                    "summary": self.summary_path().to_string_lossy(),
                    "process": self.process_path().to_string_lossy(),
                    "artifacts": self.artifacts_dir().to_string_lossy(),
                }),
            ),
        ])
    }

    fn update_summary(&mut self, event: &Value) {
        inc_summary_u64(&mut self.summary, "event_count", 1);
        if let Some(duration_ms) = event.get("duration_ms").and_then(Value::as_u64) {
            inc_summary_u64(&mut self.summary, "total_latency_ms", duration_ms);
        }
        let name = event
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let kind = event
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let attrs = event
            .get("attributes")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if event.get("status").and_then(Value::as_str) == Some("error") {
            inc_summary_u64(&mut self.summary, "error_count", 1);
        }
        match name {
            "run.finished" => {
                self.summary.insert(
                    "status".to_string(),
                    attrs
                        .get("status")
                        .cloned()
                        .unwrap_or_else(|| json!("completed")),
                );
                self.summary.insert(
                    "ended_at_ms".to_string(),
                    event.get("timestamp_ms").cloned().unwrap_or(Value::Null),
                );
            }
            "run.failed" => {
                self.summary.insert("status".to_string(), json!("failed"));
                self.summary.insert(
                    "ended_at_ms".to_string(),
                    event.get("timestamp_ms").cloned().unwrap_or(Value::Null),
                );
            }
            "step.finished" => inc_summary_u64(&mut self.summary, "step_count", 1),
            "model.call.finished" => {
                inc_summary_u64(&mut self.summary, "model_call_count", 1);
                inc_summary_u64(
                    &mut self.summary,
                    "total_input_tokens",
                    attrs
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                );
                inc_summary_u64(
                    &mut self.summary,
                    "total_output_tokens",
                    attrs
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                );
                inc_summary_u64(
                    &mut self.summary,
                    "total_reasoning_tokens",
                    attrs
                        .get("reasoning_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                );
                inc_summary_u64(
                    &mut self.summary,
                    "total_cache_read_tokens",
                    attrs
                        .get("cache_read_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                );
                inc_summary_u64(
                    &mut self.summary,
                    "total_cache_write_tokens",
                    attrs
                        .get("cache_write_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                );
                inc_summary_f64(
                    &mut self.summary,
                    "total_cost",
                    attrs
                        .get("cost")
                        .and_then(Value::as_f64)
                        .unwrap_or_default(),
                );
            }
            "tool.call.finished" => {
                inc_summary_u64(&mut self.summary, "tool_call_count", 1);
                match tool_source(&attrs) {
                    "mcp" => inc_summary_u64(&mut self.summary, "mcp_call_count", 1),
                    "skill" => inc_summary_u64(&mut self.summary, "skill_call_count", 1),
                    "local_tool" | "local" => {
                        inc_summary_u64(&mut self.summary, "local_tool_call_count", 1)
                    }
                    _ => {}
                }
                if attrs.get("output_path").is_some() {
                    inc_summary_u64(&mut self.summary, "artifact_count", 1);
                }
            }
            "artifact.created" => inc_summary_u64(&mut self.summary, "artifact_count", 1),
            "runtime.warning" => inc_summary_u64(&mut self.summary, "runtime_warning_count", 1),
            _ if kind == "artifact" => inc_summary_u64(&mut self.summary, "artifact_count", 1),
            _ => {}
        }
        let ended_at = self
            .summary
            .get("ended_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or_else(now_ms);
        let started_at = self
            .summary
            .get("started_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or(ended_at);
        self.summary.insert(
            "duration_ms".to_string(),
            json!(ended_at.saturating_sub(started_at)),
        );
    }

    fn write_summary(&self) -> SessionResult<()> {
        write_json(&self.summary_path(), &Value::Object(self.summary.clone()))
    }

    fn write_process_note(&self, message: &str) -> SessionResult<()> {
        let path = self.process_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let existing = if path.exists() {
            fs::read_to_string(&path)?
        } else {
            "# Trace Process\n\n".to_string()
        };
        fs::write(
            path,
            format!("{}\n- {}: {}\n", existing.trim_end(), now_ms(), message),
        )?;
        Ok(())
    }
}

#[must_use]
pub fn load_trace_config(options: Option<&Value>) -> TraceConfig {
    let raw = options
        .and_then(|value| value.get("trace"))
        .and_then(Value::as_object);
    TraceConfig {
        enabled: raw
            .and_then(|items| items.get("enabled"))
            .is_none_or(|value| bool_option(value, true)),
        root_dir: raw
            .and_then(|items| items.get("root_dir").or_else(|| items.get("jsonl_dir")))
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_TRACE_ROOT)
            .to_string(),
        keep_events: raw
            .and_then(|items| items.get("keep_events"))
            .is_none_or(|value| bool_option(value, true)),
        max_events: raw
            .and_then(|items| items.get("max_events"))
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_TRACE_MAX_EVENTS),
        write_summary: raw
            .and_then(|items| items.get("write_summary"))
            .is_none_or(|value| bool_option(value, true)),
        exporters: raw
            .and_then(|items| items.get("exporters"))
            .and_then(Value::as_object)
            .map(|items| items.clone().into_iter().collect())
            .unwrap_or_default(),
    }
}

pub fn load_trace_events(path: impl AsRef<Path>) -> SessionResult<Vec<Value>> {
    read_jsonl(path.as_ref())
}

pub fn load_trace_summary(path: impl AsRef<Path>) -> SessionResult<Value> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

#[must_use]
pub fn render_trace_summary(summary: &Value) -> String {
    let cost = summary
        .get("total_cost")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    format!(
        "Run: {}\nStatus: {}\nDuration: {}ms\nEvents: {}\nSteps: {}\nModel calls: {}\nTool calls: {}\nMCP calls: {}\nSkill calls: {}\nTokens: {}/{}\nReasoning tokens: {}\nCache tokens: read={} write={}\nCost: {:.6}\nErrors: {}\n",
        string_field(summary, "run_id"),
        string_field(summary, "status"),
        u64_field(summary, "duration_ms"),
        u64_field(summary, "event_count"),
        u64_field(summary, "step_count"),
        u64_field(summary, "model_call_count"),
        u64_field(summary, "tool_call_count"),
        u64_field(summary, "mcp_call_count"),
        u64_field(summary, "skill_call_count"),
        u64_field(summary, "total_input_tokens"),
        u64_field(summary, "total_output_tokens"),
        u64_field(summary, "total_reasoning_tokens"),
        u64_field(summary, "total_cache_read_tokens"),
        u64_field(summary, "total_cache_write_tokens"),
        cost,
        u64_field(summary, "error_count"),
    )
}

pub fn check_trace_run(run_dir: impl AsRef<Path>) -> SessionResult<Value> {
    let run_path = run_dir.as_ref();
    let trace_path = run_path.join("trace.jsonl");
    let summary_path = run_path.join("summary.json");
    let mut errors = Vec::new();
    let events = if trace_path.exists() {
        read_jsonl(&trace_path)?
    } else {
        errors.push("missing trace.jsonl".to_string());
        Vec::new()
    };
    let summary = if summary_path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&summary_path)?)?
    } else {
        errors.push("missing summary.json".to_string());
        json!({})
    };
    let names = events
        .iter()
        .filter_map(|event| event.get("event").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let seqs = events
        .iter()
        .filter_map(|event| event.get("seq").and_then(Value::as_u64))
        .collect::<Vec<_>>();
    if events.is_empty() {
        errors.push("trace has no events".to_string());
    }
    if !seqs.is_empty() && seqs != (1..=seqs.len() as u64).collect::<Vec<_>>() {
        errors.push("event seq values are not contiguous from 1".to_string());
    }
    if !names.contains(&"run.started") {
        errors.push("missing run.started".to_string());
    }
    if !names
        .iter()
        .any(|name| matches!(*name, "run.finished" | "run.failed"))
    {
        errors.push("missing terminal run event".to_string());
    }
    if !names.contains(&"step.started") {
        errors.push("missing step.started".to_string());
    }
    if !names.contains(&"step.finished") {
        errors.push("missing step.finished".to_string());
    }
    if !names.contains(&"model.call.started") {
        errors.push("missing model.call.started".to_string());
    }
    if !names.contains(&"model.call.finished") {
        errors.push("missing model.call.finished".to_string());
    }
    if summary
        .get("event_count")
        .and_then(Value::as_u64)
        .is_some_and(|count| count != events.len() as u64)
    {
        errors.push("summary event_count does not match trace length".to_string());
    }
    Ok(json!({
        "ok": errors.is_empty(),
        "run_id": summary.get("run_id").cloned().unwrap_or(Value::Null),
        "event_count": events.len(),
        "errors": errors,
    }))
}
