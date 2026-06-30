#[derive(Clone, Debug)]
pub struct FileSessionStore {
    pub root: PathBuf,
}

impl FileSessionStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn from_options(options: Option<&Value>, base_dir: Option<&Path>) -> Option<Self> {
        let raw = options
            .and_then(|value| value.get(SESSION_STORE_METADATA_KEY))
            .cloned()
            .unwrap_or_else(|| json!({}));
        if raw == Value::Bool(false) {
            return None;
        }
        let object = raw.as_object();
        if object
            .and_then(|items| items.get("enabled"))
            .is_some_and(|value| !bool_option(value, true))
        {
            return None;
        }
        let root_raw = object
            .and_then(|items| items.get("root_dir"))
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_SESSION_STORE_ROOT);
        let mut root = PathBuf::from(root_raw);
        if !root.is_absolute() {
            root = base_dir.unwrap_or_else(|| Path::new(".")).join(root);
        }
        Some(Self::new(root))
    }
}

include!("file_store/run.rs");
include!("file_store/events.rs");
include!("file_store/parts.rs");
include!("file_store/messages.rs");
include!("file_store/state.rs");
include!("file_store/summary.rs");
include!("file_store/paths.rs");
