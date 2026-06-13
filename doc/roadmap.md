# Roadmap

OpenAgent is usable as a core runtime, but several areas are still intentionally small.

## Current Status

Implemented:

- streaming agent loop;
- built-in workspace tools;
- permission rulesets;
- OpenAI-compatible provider;
- MCP discovery and remote tool bridge;
- context budget, structured compaction, file context, and context pack trace;
- local, optional remote sandbox, Terminal-Bench, and Harbor runtime paths;
- JSONL-friendly observability and eval/replay support;
- optional Langfuse trace export and eval score export.
- P0 file-backed session ledger for run/message/step/tool events, latest session state, and eval report references.
- Decoupled swarm/function kernel with function runners, subprocess runners, HTTP runners, A2A runners, YAML config, multi-runner dispatch, an OpenAgent runner adapter, local trace lineage, optional Langfuse export, opt-in worker workspace isolation, merge-back conflict review, file-backed run state receipts, and resumable coordinator policy.

Not complete:

- full OpenAgent session crash recovery, compaction-boundary restore, and database-backed session history are not complete;
- memory tools are process-local, not cross-session long-term memory;
- CLI and Web Console are outside the public core;
- ContextPackBuilder is trace-first, not yet the only model-message assembly path;
- step budgeting is static, so complex tasks can spend too many turns on confirmation and push required closeout artifacts past the final tool-enabled step.
- tool execution now has runtime scheduling metadata and a batch planner, but AgentLoop still executes tool calls serially.
- the swarm kernel does not yet have coordinator-level merge approval policy, resumable team adapters, or streaming A2A support.

## Next Milestones

1. Make `ContextPackBuilder` the single model-message assembly path.
2. Extend `SessionStore` from P0 file ledger to resume/crash recovery, pause state, compaction boundaries, and long-term indexes.
3. Add Langfuse-backed eval iteration: dataset sync, experiment grouping, and dashboard runbooks.
4. Add adaptive step budgeting: task-complexity defaults, remaining-step warnings, closeout protection for required artifacts, and read-only-loop detection. Track this work in [Step Budget Optimization](step-budget-optimization.md).
5. Connect the tool batch planner to AgentLoop trace, then enable read-only concurrent execution. Track this work in [Tool Optimization](tool-optimization.md).
6. Add resumable team adapters, coordinator-level merge approval, and streaming A2A support on top of the decoupled [Swarm Function Kernel](swarm.md).
7. Split optional dependencies for MCP, sandbox, and benchmark integrations.
8. Publish reproducible benchmark reports for Terminal-Bench and Harbor.
9. Rebuild CLI/Web Console as a separate demo package with sanitized config templates.

## Documentation Policy

Do not add long one-off design documents to the repo. Update `architecture.md`, `context.md`, `operations.md`, or this roadmap instead.
