use super::catalog::normalize_models_catalog;
use super::*;

#[derive(Clone, Debug)]
pub(super) struct ModelsCacheLoad {
    pub(super) value: Value,
    pub(super) path: PathBuf,
    pub(super) snapshot_path: PathBuf,
    pub(super) status: String,
    pub(super) refreshed: bool,
    pub(super) stale: bool,
    pub(super) fallback: bool,
    pub(super) error: Option<String>,
}

impl ModelsCacheLoad {
    pub(super) fn to_value(&self) -> Value {
        json!({
            "path": self.path.to_string_lossy(),
            "snapshot_path": self.snapshot_path.to_string_lossy(),
            "status": self.status,
            "refreshed": self.refreshed,
            "stale": self.stale,
            "fallback": self.fallback,
            "error": self.error,
            "source": self.value.get("source").cloned().unwrap_or(Value::Null),
        })
    }
}

struct ModelsCacheLock {
    path: PathBuf,
}

impl ModelsCacheLock {
    fn acquire(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if model_cache_lock_is_stale(&path) {
            let _ = fs::remove_file(&path);
        }
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = writeln!(
                    file,
                    "pid={} created_at_ms={}",
                    std::process::id(),
                    now_ms_cli()
                );
                Ok(Self { path })
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(format!(
                "models cache refresh locked: {}",
                path.to_string_lossy()
            )),
            Err(error) => Err(format!("failed to lock models cache: {error}")),
        }
    }
}

impl Drop for ModelsCacheLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) fn models_cache_path() -> PathBuf {
    env::var("OPENAGENT_MODELS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".cache/openagent/models.json"))
}

pub(super) fn ensure_models_cache(args: &[String]) -> ModelsCacheLoad {
    let path = models_cache_path();
    let snapshot_path = models_cache_snapshot_path();
    let force_refresh = has_flag(args, &["--refresh"]);
    let offline = has_flag(args, &["--offline"]);
    let ttl_seconds = models_cache_ttl_seconds(args);
    let current = load_models_cache_file(&path, ttl_seconds);
    let current_stale = current
        .as_ref()
        .is_none_or(|value| models_cache_is_stale(value, ttl_seconds));
    let should_refresh = force_refresh || (current.is_some() && current_stale);
    if !offline && should_refresh {
        match refresh_models_cache(args) {
            Ok(value) => {
                let stale = models_cache_is_stale(&value, ttl_seconds);
                return ModelsCacheLoad {
                    value,
                    path,
                    snapshot_path,
                    status: "refreshed".to_string(),
                    refreshed: true,
                    stale,
                    fallback: false,
                    error: None,
                };
            }
            Err(error) => {
                if let Some(value) = current {
                    return ModelsCacheLoad {
                        value,
                        path,
                        snapshot_path,
                        status: "stale_refresh_failed".to_string(),
                        refreshed: false,
                        stale: true,
                        fallback: true,
                        error: Some(error),
                    };
                }
                if let Some(value) = load_models_cache_file(&snapshot_path, ttl_seconds) {
                    return ModelsCacheLoad {
                        value,
                        path,
                        snapshot_path,
                        status: "snapshot_fallback".to_string(),
                        refreshed: false,
                        stale: true,
                        fallback: true,
                        error: Some(error),
                    };
                }
                return ModelsCacheLoad {
                    value: empty_models_cache(ttl_seconds),
                    path,
                    snapshot_path,
                    status: "empty_refresh_failed".to_string(),
                    refreshed: false,
                    stale: true,
                    fallback: true,
                    error: Some(error),
                };
            }
        }
    }
    if let Some(value) = current {
        return ModelsCacheLoad {
            value,
            path,
            snapshot_path,
            status: if current_stale { "stale" } else { "hit" }.to_string(),
            refreshed: false,
            stale: current_stale,
            fallback: false,
            error: None,
        };
    }
    if let Some(value) = load_models_cache_file(&snapshot_path, ttl_seconds) {
        return ModelsCacheLoad {
            value,
            path,
            snapshot_path,
            status: "snapshot_fallback".to_string(),
            refreshed: false,
            stale: true,
            fallback: true,
            error: None,
        };
    }
    ModelsCacheLoad {
        value: empty_models_cache(ttl_seconds),
        path,
        snapshot_path,
        status: "empty".to_string(),
        refreshed: false,
        stale: true,
        fallback: true,
        error: None,
    }
}

fn refresh_models_cache(args: &[String]) -> Result<Value, String> {
    let path = models_cache_path();
    let _lock = ModelsCacheLock::acquire(models_cache_lock_path())?;
    let url = models_source_url(args);
    let endpoint = join_url(&url, "api.json");
    let raw = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(
            value_for(args, &["--timeout-s"])
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(20),
        ))
        .build()
        .map_err(|error| error.to_string())?
        .get(&endpoint)
        .send()
        .map_err(|error| format!("failed to fetch models cache: {error}"))?
        .text()
        .map_err(|error| format!("failed to read models cache: {error}"))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("models cache response was not JSON: {error}"))?;
    let normalized = normalize_models_catalog(&value, &endpoint, models_cache_ttl_seconds(args));
    write_json_file(&path, &normalized)?;
    write_json_file(&models_cache_snapshot_path(), &normalized)?;
    Ok(normalized)
}

fn load_models_cache_file(path: &Path, ttl_seconds: u64) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    if value
        .get("schema_version")
        .and_then(Value::as_str)
        .is_some_and(|schema| schema == "openagent.models_cache.v1")
    {
        return Some(value);
    }
    Some(normalize_models_catalog(&value, "local-cache", ttl_seconds))
}

pub(super) fn empty_models_cache(ttl_seconds: u64) -> Value {
    json!({
        "schema_version": "openagent.models_cache.v1",
        "source": {
            "url": null,
            "fetched_at_ms": 0,
            "ttl_seconds": ttl_seconds,
            "provider_count": 0,
            "model_count": 0,
            "raw_schema": "empty",
        },
        "providers": {},
        "catalog": [],
    })
}

fn models_cache_snapshot_path() -> PathBuf {
    models_cache_path().with_extension("snapshot.json")
}

fn models_cache_lock_path() -> PathBuf {
    models_cache_path().with_extension("lock")
}

fn models_source_url(args: &[String]) -> String {
    value_for(args, &["--models-url"])
        .or_else(|| env::var("OPENAGENT_MODELS_URL").ok())
        .unwrap_or_else(|| "https://models.dev".to_string())
}

fn models_cache_ttl_seconds(args: &[String]) -> u64 {
    value_for(args, &["--ttl-seconds"])
        .or_else(|| env::var("OPENAGENT_MODELS_TTL_SECONDS").ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(24 * 60 * 60)
}

fn models_cache_is_stale(cache: &Value, ttl_seconds: u64) -> bool {
    let fetched_at = cache
        .get("source")
        .and_then(|source| source.get("fetched_at_ms"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if fetched_at == 0 {
        return true;
    }
    now_ms_cli().saturating_sub(fetched_at) > ttl_seconds.saturating_mul(1000)
}

fn model_cache_lock_is_stale(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|duration| duration > Duration::from_secs(120))
        .unwrap_or(false)
}
