# Rust Rewrite Parity Matrix

This matrix assigns current Python production surfaces to their Rust owners and
verification gates. It is the Goal 0 inventory for the rewrite.

| Python Surface | Current Path | Rust Owner | First Goal | Final Removal Gate |
| --- | --- | --- | --- | --- |
| Shared protocol types | `src/openagent/core/types.py` | `openagent-protocol` | Goal 2 | Rust serde fixtures match golden JSON. |
| Provider payload materialization | `src/openagent/core/message_materializer.py` | `openagent-protocol` / `openagent-provider` | Goal 2 | OpenAI-compatible payload fixtures match. |
| Permission rules | `src/openagent/core/permission/` | `openagent-core` | Goal 6 | Ruleset fixtures and permission manager tests pass. |
| Tool definitions/registry | `src/openagent/core/tool/definition.py`, `registry.py`, `toolkit.py` | `openagent-tools` | Goal 5 | Tool schema, middleware, and registry parity tests pass. |
| Built-in workspace tools | `src/openagent/core/tool/builtin/` | `openagent-tools` | Goal 5 | File/shell/search/todo/memory/question fixtures and tests pass. |
| Workspace runtime | `src/openagent/core/execution/` | `openagent-tools` | Goal 5 | Local runtime path safety and command execution tests pass. |
| Context budget/messages/pack | `src/openagent/core/context_*.py` | `openagent-core` | Goal 6 | Context trimming, compaction, and pack traces pass parity tests. |
| Instructions and skills | `src/openagent/core/instructions.py`, `skill/` | `openagent-core` / `openagent-tools` | Goal 6 | Skill discovery and instruction loading tests pass. |
| Provider adapters | `src/openagent/core/provider/` | `openagent-provider` | Goal 7 | Mock OpenAI/Anthropic streaming tests pass. |
| Agent loop | `src/openagent/core/loop/` | `openagent-core` | Goal 8 | Multi-step stream-event fixtures and loop tests pass. |
| Session store | `src/openagent/core/session/` | `openagent-session` | Goal 4 | File-backed ledger/state fixtures pass. |
| Trace and observability | `src/openagent/core/trace/`, `observability.py`, `runtime_logging.py`, `runtime_warnings.py` | `openagent-session` / `openagent-core` | Goal 4 | JSONL trace, redaction, warning, and exporter tests pass. |
| MCP config/runtime | `src/openagent/core/mcp/` | `openagent-mcp` or `openagent-tools` | Goal 9 | Config/discovery/auth/tool-call tests pass. |
| Eval runner and CI gate | `src/openagent/core/eval/` | `openagent-eval` | Goal 13 | Eval report and regression gate tests pass. |
| CLI | `src/openagent/cli/` | `openagent-cli` | Goal 10 | CLI JSON/table snapshot and smoke tests pass. |
| TUI | `src/openagent/tui/` | `openagent-tui` | Goal 11 | Local and remote attach smoke tests pass. |
| App Bridge server | `src/openagent/app_server/` | `openagent-app-server` | Goal 11 | REST/SSE/control route tests pass. |
| Swarm kernel | `src/swarm/` | `openagent-swarm` | Goal 3 | Function/subprocess/http/a2a runner parity tests pass. |
| OpenAgent swarm adapter | `src/openagent/integrations/swarm.py` | `openagent-swarm` / `openagent-core` | Goal 3 / Goal 8 | Rust OpenAgent runner adapter tests pass. |
| Benchmark adapters | `src/openagent/integrations/terminal_bench.py`, `harbor.py` | `openagent-eval` | Goal 13 | Adapter smoke and report schema tests pass. |
| SDK HTTP runtime client | `src/openagent/sdk/http_runtime.py` | `openagent-app-server-client` | Goal 11 / Goal 12 | Client/server API parity tests pass. |
| HTTP runtime service | `../openagent-runtime-http/src/openagent_runtime_http/` | `openagent-http-runtime` | Goal 12 | API parity, auth, quota, SSE, Docker smoke pass. |

## Fixture Groups

Goal 0 captures these stable groups:

- `core_protocol.json`: shared types, stream events, materialized provider
  payloads, runtime option filtering.
- `permission_rulesets.json`: `FULL`, `READONLY`, `PLAN_ONLY`, and `NONE`.
- `tool_definition_schema.json`: JSON schema generated from tool parameter
  dataclasses.
- `swarm_protocol.json`: swarm spec/result/descriptor/budget protocol.
- `context_state.json`: structured compaction parsing/rendering behavior.

Later goals may add more fixtures, but they must remain deterministic and
network-free unless explicitly marked as smoke fixtures.
