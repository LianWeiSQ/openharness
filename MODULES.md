# OpenHarness Rust Modules

The Rust workspace is grouped by product boundary. `src/` owns the core engine
and internal library crates; user-facing or operational surfaces stay as clear
top-level modules.

| Directory | Crate |
| --- | --- |
| `src` | `openagent-core` |
| `src/protocol` | `openagent-protocol` |
| `src/tools` | `openagent-tools` |
| `src/provider` | `openagent-provider` |
| `src/session` | `openagent-session` |
| `src/mcp` | `openagent-mcp` |
| `swarm` | `openagent-swarm` |
| `eval` | `openagent-eval` |
| `cli` | `openagent-cli` |
| `runtime/app-server` | `openagent-app-server` |
| `runtime/app-server-client` | `openagent-app-server-client` |
| `runtime/tui` | `openagent-tui` |
| `runtime/http` | `openagent-http-runtime` |
