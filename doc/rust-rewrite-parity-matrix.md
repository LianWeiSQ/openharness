# Rust Rewrite Parity Matrix

This matrix records the final Rust ownership after the Goal 0-14 rewrite. The
former Python production tree has been removed; deterministic golden JSON files
under `tests/golden/rust_rewrite/` remain as compatibility artifacts.

| Runtime Surface | Rust Owner | Golden Fixture / Gate |
| --- | --- | --- |
| Shared protocol types and stream events | `openagent-protocol` | `core_protocol.json`, protocol fixture tests |
| Permission rulesets | `openagent-protocol`, `openagent-core` | `permission_rulesets.json`, permission tests |
| Tool schemas and built-in workspace tools | `openagent-tools` | `tool_definition_schema.json`, `tool_runtime.json` |
| Context budget, compaction, instructions, skills | `openagent-core`, `openagent-tools` | `core_context_policy.json` |
| Provider metadata and stream normalization | `openagent-provider` | `provider_adapters.json` |
| Agent loop behavior | `openagent-core` | `agent_loop.json` |
| Session store, trace, observability, runtime logs | `openagent-session`, `openagent-core` | `session_trace_observability.json` |
| MCP config, discovery, bridge, result normalization | `openagent-mcp` | `mcp_runtime.json` |
| Swarm runner orchestration | `openagent-swarm` | `swarm_protocol.json`, swarm runtime tests |
| CLI command layer | `openagent-cli` | `cli_commands.json`, compiled binary smoke tests |
| App Bridge server protocol/state | `openagent-app-server` | `app_bridge_tui.json` |
| App Bridge client helpers | `openagent-app-server-client` | `app_bridge_tui.json` |
| TUI control state | `openagent-tui` | `app_bridge_tui.json` |
| HTTP runtime binary contracts | `openagent-http-runtime` | `http_runtime.json`, Dockerfile contract |
| Eval, CI gate, benchmark adapters | `openagent-eval` | `eval_integrations.json` |

## Final Gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git ls-files '*.py' pyproject.toml
```

The last command must produce no tracked files.
