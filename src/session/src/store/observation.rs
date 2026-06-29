#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ObservationConfig {
    pub enabled: bool,
    pub keep_events: bool,
    pub jsonl: bool,
    pub jsonl_dir: String,
    pub max_events: u64,
    pub input_preview_chars: usize,
    pub include_traceback: bool,
}

impl Default for ObservationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_events: true,
            jsonl: false,
            jsonl_dir: DEFAULT_OBSERVABILITY_JSONL_DIR.to_string(),
            max_events: DEFAULT_MAX_EVENTS,
            input_preview_chars: DEFAULT_INPUT_PREVIEW_CHARS,
            include_traceback: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ObservationTraceRecord {
    pub trace_id: String,
    pub session_id: String,
    pub run_id: String,
    pub agent_name: String,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
    pub workspace: Option<String>,
    pub started_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ObservationEvent {
    pub event_id: String,
    pub trace_id: String,
    pub run_id: String,
    pub session_id: String,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub timestamp_ms: u64,
    pub duration_ms: Option<u64>,
    pub status: String,
    pub attributes: BTreeMap<String, Value>,
}

pub struct ObservationRecorder<'a> {
    pub trace: ObservationTraceRecord,
    pub config: ObservationConfig,
    pub base_dir: PathBuf,
    session_metadata: &'a mut BTreeMap<String, Value>,
}

impl<'a> ObservationRecorder<'a> {
    pub fn new(
        trace: ObservationTraceRecord,
        config: Option<ObservationConfig>,
        base_dir: impl Into<PathBuf>,
        session_metadata: &'a mut BTreeMap<String, Value>,
    ) -> Self {
        let mut recorder = Self {
            trace,
            config: config.unwrap_or_default(),
            base_dir: base_dir.into(),
            session_metadata,
        };
        if recorder.config.enabled {
            recorder.ensure_metadata_root();
        }
        recorder
    }

    pub fn event(
        &mut self,
        name: &str,
        kind: &str,
        attributes: BTreeMap<String, Value>,
        options: ObservationEventOptions,
    ) -> SessionResult<Option<ObservationEvent>> {
        if !self.config.enabled {
            return Ok(None);
        }
        let event = ObservationEvent {
            event_id: options.event_id.unwrap_or_else(|| new_id("event")),
            trace_id: self.trace.trace_id.clone(),
            run_id: self.trace.run_id.clone(),
            session_id: self.trace.session_id.clone(),
            span_id: options.span_id,
            parent_span_id: options.parent_span_id,
            name: name.to_string(),
            kind: kind.to_string(),
            timestamp_ms: options.timestamp_ms.unwrap_or_else(now_ms),
            duration_ms: options.duration_ms,
            status: if options.status == "error" {
                "error"
            } else {
                "ok"
            }
            .to_string(),
            attributes: sanitize_value_map(attributes, DEFAULT_FIELD_PREVIEW_CHARS),
        };
        self.record(&event)?;
        Ok(Some(event))
    }

    fn ensure_metadata_root(&mut self) {
        let jsonl_path = if self.config.jsonl {
            Some(self.jsonl_path().to_string_lossy().to_string())
        } else {
            None
        };
        let root = self
            .session_metadata
            .entry(OBSERVABILITY_METADATA_KEY.to_string())
            .or_insert_with(|| json!({}));
        if !root.is_object() {
            *root = json!({});
        }
        if let Some(object) = root.as_object_mut() {
            object.insert(
                "trace".to_string(),
                serde_json::to_value(&self.trace).unwrap_or(Value::Null),
            );
            object
                .entry("events".to_string())
                .or_insert_with(|| json!([]));
            object
                .entry("event_count".to_string())
                .or_insert_with(|| json!(0));
            object.insert(
                "jsonl_path".to_string(),
                jsonl_path.map_or(Value::Null, Value::String),
            );
        }
    }

    fn record(&mut self, event: &ObservationEvent) -> SessionResult<()> {
        self.ensure_metadata_root();
        if let Some(Value::Object(root)) = self.session_metadata.get_mut(OBSERVABILITY_METADATA_KEY)
        {
            let count = root
                .get("event_count")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                + 1;
            root.insert("event_count".to_string(), json!(count));
            root.insert("last_event_at_ms".to_string(), json!(event.timestamp_ms));
            if self.config.keep_events {
                let mut events = root
                    .get("events")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                events.push(serde_json::to_value(event)?);
                let max_events = self.config.max_events.max(1) as usize;
                if events.len() > max_events {
                    events = events[events.len() - max_events..].to_vec();
                }
                root.insert("events".to_string(), Value::Array(events));
            }
        }
        if self.config.jsonl {
            append_jsonl(&self.jsonl_path(), event)?;
        }
        Ok(())
    }

    fn jsonl_path(&self) -> PathBuf {
        let root = PathBuf::from(&self.config.jsonl_dir);
        let root = if root.is_absolute() {
            root
        } else {
            self.base_dir.join(root)
        };
        root.join(&self.trace.session_id)
            .join(format!("{}.jsonl", self.trace.run_id))
    }
}

#[derive(Clone, Debug)]
pub struct ObservationEventOptions {
    pub event_id: Option<String>,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub duration_ms: Option<u64>,
    pub timestamp_ms: Option<u64>,
    pub status: String,
}

impl Default for ObservationEventOptions {
    fn default() -> Self {
        Self {
            event_id: None,
            span_id: None,
            parent_span_id: None,
            duration_ms: None,
            timestamp_ms: None,
            status: "ok".to_string(),
        }
    }
}

#[must_use]
pub fn load_observation_config(options: Option<&Value>) -> ObservationConfig {
    let raw = options
        .and_then(|value| value.get("observability"))
        .and_then(Value::as_object);
    ObservationConfig {
        enabled: raw
            .and_then(|items| items.get("enabled"))
            .is_none_or(|value| bool_option(value, true)),
        keep_events: raw
            .and_then(|items| items.get("keep_events"))
            .is_none_or(|value| bool_option(value, true)),
        jsonl: raw
            .and_then(|items| items.get("jsonl"))
            .is_some_and(|value| bool_option(value, false)),
        jsonl_dir: raw
            .and_then(|items| items.get("jsonl_dir"))
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_OBSERVABILITY_JSONL_DIR)
            .to_string(),
        max_events: raw
            .and_then(|items| items.get("max_events"))
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_EVENTS),
        input_preview_chars: raw
            .and_then(|items| items.get("input_preview_chars"))
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_INPUT_PREVIEW_CHARS as u64) as usize,
        include_traceback: raw
            .and_then(|items| items.get("include_traceback"))
            .is_some_and(|value| bool_option(value, false)),
    }
}
