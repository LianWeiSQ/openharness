# Architecture

OpenAgent is a Rust-only agent harness workspace for tool-using and coding
agents. The runtime is organized around a small set of durable concepts:
protocol types, an agent loop, context assembly, tool execution, permission
policy, session storage, trace/eval evidence, MCP integration, and product
surfaces such as CLI, TUI, App Bridge, and the HTTP runtime.

```text
User task
  -> Agent loop / turn runtime
  -> Context runtime
  -> Provider boundary
  -> Tool calls
  -> Permission policy
  -> Workspace / MCP / skill tools
  -> Session ledger / trace / parts
  -> CLI / TUI / HTTP runtime / App Bridge
```

## Workspace Crates

| Crate | Responsibility |
| --- | --- |
| `crates/openagent-protocol` | Shared serde protocol types and runtime contracts |
| `crates/openagent-core` | Agent loop, context, permission, policy, and skills |
| `crates/openagent-tools` | Tool registry, built-in tools, and workspace runtime |
| `crates/openagent-provider` | Provider metadata and stream normalization |
| `crates/openagent-session` | Session store, trace, observability, and replay evidence |
| `crates/openagent-mcp` | MCP config, discovery, auth, and tool bridge contracts |
| `crates/openagent-swarm` | Agent-agnostic swarm runner orchestration |
| `crates/openagent-eval` | Eval runner, CI gate, and benchmark integrations |
| `crates/openagent-cli` | `openagent` command-line binary |
| `crates/openagent-tui` | Local and remote terminal UI state |
| `crates/openagent-app-server` | App Bridge server protocol and state |
| `crates/openagent-app-server-client` | App Bridge client helpers |
| `crates/openagent-http-runtime` | HTTP runtime binary and API contracts |

## Tool Flow

The model receives tool schemas through the provider boundary. If it emits a
tool call, OpenAgent:

1. validates the call against registered tool definitions;
2. evaluates the active permission ruleset;
3. executes the tool through the appropriate runtime or bridge;
4. persists the tool result into session/trace records;
5. feeds the result back into the next model step or turn.

Workspace tools are responsible for file, shell, search, and edit operations.
MCP and skill tools are bridged through their own contracts while preserving a
common tool-call shape for permission checks, trace records, and eval replay.

## Context Runtime

OpenAgent treats context as runtime state rather than a single prompt string.
The context path tracks instruction assets, file assets, context budget,
structured compaction, context pack snapshots, and session parts. The goal is
to make context selection recoverable and debuggable: the runtime should be
able to explain which items were included, which were dropped under budget
pressure, and which assets changed before a resumed turn.

See [`context.md`](context.md) for the context persistence stages.

## Session And Trace

The session layer stores restorable state, append-only events, durable parts,
trace summaries, usage, warnings, and replay/eval evidence. This layer is the
bridge between raw model messages and product/runtime observability: product
surfaces can inspect the session, while eval tooling can replay or score the
same run evidence.

See [`operations.md`](operations.md) for runtime events and operational data.

## Provider Boundary

Provider-specific SDKs and wire formats should stay outside the agent loop.
The loop consumes normalized model text, tool calls, usage, and stream events.
This keeps provider changes from leaking into tool execution, context
assembly, permission policy, and session persistence.

## Public Scope

The repository is the Rust harness workspace. Legacy Python runtime code and
package metadata were removed during the Rust rewrite; compatibility fixtures
remain only as golden artifacts where they are useful for regression checks.
