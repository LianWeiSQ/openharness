use std::{
    error::Error,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use openagent_app_server_client::{RemoteAuth, RemoteRuntimeClient};
use openagent_http_runtime::http_runtime_fixture;
use serde_json::Value;

type FakeProviderServer = (u16, thread::JoinHandle<()>, Arc<Mutex<Vec<String>>>);

include!("http_runtime/smoke_routes.rs");
include!("http_runtime/client_sessions.rs");
include!("http_runtime/provider_turns.rs");
include!("http_runtime/task_subagents_basic.rs");
include!("http_runtime/task_subagents_governance.rs");
include!("http_runtime/task_subagents_background.rs");
include!("http_runtime/task_subagents_locks.rs");
include!("http_runtime/interactions.rs");
include!("http_runtime/live_sse.rs");
include!("http_runtime/fixtures.rs");
include!("http_runtime/providers.rs");
include!("http_runtime/http_helpers.rs");
