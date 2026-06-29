#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeLoggingConfig {
    pub enabled: bool,
    pub keep_records: bool,
    pub jsonl: bool,
    pub jsonl_dir: String,
    pub max_records: u64,
    pub input_preview_chars: usize,
    pub level: String,
    pub structured_logging: bool,
    pub logger_name: String,
    pub include_context: bool,
}

impl Default for RuntimeLoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_records: true,
            jsonl: false,
            jsonl_dir: DEFAULT_LOGGING_JSONL_DIR.to_string(),
            max_records: DEFAULT_MAX_EVENTS,
            input_preview_chars: DEFAULT_INPUT_PREVIEW_CHARS,
            level: "INFO".to_string(),
            structured_logging: false,
            logger_name: "openagent.runtime".to_string(),
            include_context: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeLogRecord {
    pub log_id: String,
    pub timestamp_ms: u64,
    pub level: String,
    pub message: String,
    pub category: String,
    pub session_id: String,
    pub run_id: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub attributes: BTreeMap<String, Value>,
}

pub struct RuntimeLogger<'a> {
    session_id: String,
    session_metadata: &'a mut BTreeMap<String, Value>,
    config: RuntimeLoggingConfig,
    base_dir: PathBuf,
    run_id: Option<String>,
    trace_id: Option<String>,
}

impl<'a> RuntimeLogger<'a> {
    pub fn new(
        session_id: impl Into<String>,
        session_metadata: &'a mut BTreeMap<String, Value>,
        config: Option<RuntimeLoggingConfig>,
        base_dir: impl Into<PathBuf>,
        run_id: Option<String>,
        trace_id: Option<String>,
    ) -> Self {
        let mut logger = Self {
            session_id: session_id.into(),
            session_metadata,
            config: config.unwrap_or_default(),
            base_dir: base_dir.into(),
            run_id,
            trace_id,
        };
        if logger.config.enabled {
            logger.ensure_metadata_root();
        }
        logger
    }

    pub fn log(
        &mut self,
        level: &str,
        message: &str,
        category: &str,
        attributes: BTreeMap<String, Value>,
        timestamp_ms: Option<u64>,
    ) -> SessionResult<Option<RuntimeLogRecord>> {
        if !self.config.enabled {
            return Ok(None);
        }
        let normalized_level = normalize_level(level);
        if level_number(&normalized_level) < level_number(&self.config.level) {
            return Ok(None);
        }
        let record = RuntimeLogRecord {
            log_id: new_id("log"),
            timestamp_ms: timestamp_ms.unwrap_or_else(now_ms),
            level: normalized_level,
            message: message.to_string(),
            category: category.to_string(),
            session_id: self.session_id.clone(),
            run_id: self.run_id.clone(),
            trace_id: self.trace_id.clone(),
            span_id: None,
            attributes: sanitize_value_map(attributes, DEFAULT_FIELD_PREVIEW_CHARS),
        };
        self.record(&record)?;
        Ok(Some(record))
    }

    fn ensure_metadata_root(&mut self) {
        let jsonl_path = if self.config.jsonl {
            Some(self.jsonl_path().to_string_lossy().to_string())
        } else {
            None
        };
        let root = self
            .session_metadata
            .entry(LOGGING_METADATA_KEY.to_string())
            .or_insert_with(|| json!({}));
        if !root.is_object() {
            *root = json!({});
        }
        if let Some(object) = root.as_object_mut() {
            object
                .entry("records".to_string())
                .or_insert_with(|| json!([]));
            object
                .entry("record_count".to_string())
                .or_insert_with(|| json!(0));
            object.insert("level".to_string(), json!(self.config.level));
            object.insert("run_id".to_string(), json!(self.run_id));
            object.insert("trace_id".to_string(), json!(self.trace_id));
            object.insert(
                "jsonl_path".to_string(),
                jsonl_path.map_or(Value::Null, Value::String),
            );
        }
    }

    fn record(&mut self, record: &RuntimeLogRecord) -> SessionResult<()> {
        self.ensure_metadata_root();
        if let Some(Value::Object(root)) = self.session_metadata.get_mut(LOGGING_METADATA_KEY) {
            let count = root
                .get("record_count")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                + 1;
            root.insert("record_count".to_string(), json!(count));
            root.insert("last_log_at_ms".to_string(), json!(record.timestamp_ms));
            if self.config.keep_records {
                let mut records = root
                    .get("records")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                records.push(serde_json::to_value(record)?);
                let max_records = self.config.max_records.max(1) as usize;
                if records.len() > max_records {
                    records = records[records.len() - max_records..].to_vec();
                }
                root.insert("records".to_string(), Value::Array(records));
            }
        }
        if self.config.jsonl {
            append_jsonl(&self.jsonl_path(), record)?;
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
        root.join(&self.session_id).join(format!(
            "{}.jsonl",
            self.run_id.as_deref().unwrap_or("run_unbound")
        ))
    }
}
