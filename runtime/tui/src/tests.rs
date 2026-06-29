use super::*;
use ratatui::backend::TestBackend;
use std::{
    error::Error,
    fs,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

include!("tests/state_interactions.rs");
include!("tests/remote_control.rs");
include!("tests/local_interactions.rs");
include!("tests/composer.rs");
include!("tests/config_observability.rs");
include!("tests/key_flow_sessions.rs");
include!("tests/key_flow_pickers.rs");
include!("tests/app_bridge_smoke.rs");
include!("tests/render_snapshots.rs");
include!("tests/dock_interactions.rs");
include!("tests/core_snapshot.rs");
include!("tests/helpers.rs");
