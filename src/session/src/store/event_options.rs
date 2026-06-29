#[derive(Clone, Debug)]
pub struct SessionEventOptions {
    pub kind: String,
    pub status: String,
    pub attributes: BTreeMap<String, Value>,
    pub duration_ms: Option<u64>,
    pub timestamp_ms: Option<u64>,
}

impl Default for SessionEventOptions {
    fn default() -> Self {
        Self {
            kind: "event".to_string(),
            status: "ok".to_string(),
            attributes: BTreeMap::new(),
            duration_ms: None,
            timestamp_ms: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionPartOptions {
    pub part_id: Option<String>,
    pub message_id: Option<String>,
    pub content: Option<Value>,
    pub attributes: BTreeMap<String, Value>,
    pub step_index: Option<u64>,
    pub status: String,
    pub timestamp_ms: Option<u64>,
}

impl Default for SessionPartOptions {
    fn default() -> Self {
        Self {
            part_id: None,
            message_id: None,
            content: None,
            attributes: BTreeMap::new(),
            step_index: None,
            status: "ok".to_string(),
            timestamp_ms: None,
        }
    }
}
