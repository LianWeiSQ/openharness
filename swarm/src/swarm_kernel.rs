//! Agent-agnostic swarm kernel crate for the Rust rewrite.

use std::{
    collections::BTreeMap,
    future::Future,
    path::Path,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use openagent_protocol::{
    AgentDescriptor, AgentResult, AgentSpec, ArtifactRef, FanoutBudget, PermissionMode, RunContext,
    RunLimits, RunStatus, SwarmUsage,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

pub type SwarmError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type SwarmResult<T> = Result<T, SwarmError>;
pub type FunctionFuture = Pin<Box<dyn Future<Output = ResultPayload> + Send>>;
pub type FunctionHandler = Arc<dyn Fn(AgentSpec, RunContext) -> FunctionFuture + Send + Sync>;

include!("swarm/runners.rs");
include!("swarm/config.rs");
include!("swarm/runtime.rs");
include!("swarm/config_loading.rs");
include!("swarm/transport_payloads.rs");
include!("swarm/aggregation.rs");
include!("swarm/metadata.rs");
include!("swarm/deserializers.rs");
include!("swarm/tests.rs");
