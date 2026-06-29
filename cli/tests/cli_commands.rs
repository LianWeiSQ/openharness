use std::{
    error::Error,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};

use openagent_cli::cli_commands_fixture;
use serde_json::{Value, json};

type MockServer = thread::JoinHandle<Result<(), String>>;
type DrippingSseProvider = (u16, MockServer, mpsc::Sender<()>, Arc<AtomicBool>);

include!("cli_commands/smoke.rs");
include!("cli_commands/catalog.rs");
include!("cli_commands/streaming.rs");
include!("cli_commands/task_mcp_agent.rs");
include!("cli_commands/interactions.rs");
include!("cli_commands/remote_attach.rs");
include!("cli_commands/helpers.rs");
