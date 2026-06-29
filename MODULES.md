# OpenAgent Rust Crates

Goal 1 establishes the Rust workspace boundary. These crates are intentionally
small until later goals migrate behavior from Python.

| Crate | Future owner |
| --- | --- |
| `openagent-protocol` | Shared protocol types and serde contracts |
| `openagent-core` | Agent loop, context, permission, and policy |
| `openagent-tools` | Tool registry, built-in tools, workspace runtime |
| `openagent-provider` | Model provider adapters |
| `openagent-session` | Session store, trace, observability records |
| `openagent-swarm` | Agent-agnostic swarm kernel |
| `openagent-mcp` | MCP config, auth, discovery, and tool bridge |
| `openagent-eval` | Eval runner, CI gate, benchmark integrations |
| `openagent-cli` | `openagent` CLI binary |
| `openagent-app-server` | App Bridge server |
| `openagent-app-server-client` | App Bridge client SDK |
| `openagent-tui` | Local and remote terminal UI |
| `openagent-http-runtime` | HTTP runtime service replacing FastAPI |
