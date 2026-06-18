# Documentation

OpenAgent keeps the public docs intentionally small. Start with the README, then use these pages when you need implementation-level context.

| Document | What it covers |
| --- | --- |
| [Architecture](architecture.md) | Runtime shape, core modules, tool flow, provider boundary |
| [Context Engineering](context.md) | Context budget, compaction, instructions, file context, ContextPackBuilder |
| [Operations](operations.md) | Observability, runtime logs, eval/replay, Terminal-Bench/Harbor adapters |
| [OpenCode CLI/TUI Parity Matrix](opencode-cli-tui-parity-matrix.md) | Auditable CLI/TUI gap ledger, row issues, priorities, and verification evidence |
| [Roadmap](roadmap.md) | Current gaps and the next engineering milestones |
| [Step Budget Optimization](step-budget-optimization.md) | Adaptive `max_steps`, closeout protection, and runtime warnings discovered from real traces |
| [Tool Optimization](tool-optimization.md) | Tool execution schema, builtin tool classification, and batch scheduling roadmap |
| [Swarm Function Kernel](swarm.md) | Decoupled function-mode swarm kernel, YAML config, and runner boundary |

## Maintainer Rule

Keep docs short and current. Prefer updating one of the four pages above over adding a new long design note.
