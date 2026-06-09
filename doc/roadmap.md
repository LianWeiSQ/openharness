# Roadmap

OpenAgent is usable as a core runtime, but several areas are still intentionally small.

## Current Status

Implemented:

- streaming agent loop;
- built-in workspace tools;
- permission rulesets;
- OpenAI-compatible and DashScope providers;
- MCP discovery and remote tool bridge;
- context budget, structured compaction, file context, and context pack trace;
- local, optional remote sandbox, Terminal-Bench, and Harbor runtime paths;
- JSONL-friendly observability and eval/replay support.

Not complete:

- persistent session storage is not wired into the main loop;
- memory tools are process-local, not cross-session long-term memory;
- Anthropic, Gemini, and Ollama providers are stubs;
- CLI and Web Console are outside the public core;
- ContextPackBuilder is trace-first, not yet the only model-message assembly path.

## Next Milestones

1. Make `ContextPackBuilder` the single model-message assembly path.
2. Wire persistent session storage into run start, step finish, pause, resume, and compaction.
3. Split optional dependencies for MCP, sandbox, and benchmark integrations.
4. Add provider smoke tests and clearer provider capability metadata.
5. Publish reproducible benchmark reports for Terminal-Bench and Harbor.
6. Rebuild CLI/Web Console as a separate demo package with sanitized config templates.

## Documentation Policy

Do not add long one-off design documents to the repo. Update `architecture.md`, `context.md`, `operations.md`, or this roadmap instead.
