# Context Engineering

OpenAgent treats context as runtime state, not as one giant prompt string.

## Context Sources

Current model input can include:

- session messages;
- structured work-state compaction;
- todo state;
- project instruction files such as `OPENAGENT.md`, `AGENTS.md`, `CLAUDE.md`, and `.openagent/rules/*.md`;
- file-read state with path, hash, size, preview, and change status;
- runtime metadata such as execution mode and workspace root;
- recent tool results after budget-aware trimming.

Sensitive connection details are not projected into context or tool metadata.

## Budget Strategy

Before each model call, OpenAgent estimates context size and applies a staged policy:

1. trim old tool outputs;
2. compact older conversation into a structured work state;
3. project the work state as `full`, `brief`, or `minimal`;
4. fall back to a smaller final attempt if the context is still too large.

The goal is resumability under token pressure: keep task intent, decisions, changed files, blockers, and next steps even when raw transcript history is reduced.

## Structured Work State

Compaction stores a continuation packet in `Session.metadata["context_compaction"]`. It preserves the legacy `summary` field for compatibility and adds structured fields for:

- task and goal;
- completed work;
- active files;
- decisions;
- tool findings;
- todos;
- blockers;
- next steps;
- risks and validation gaps.

Provider output is parsed best-effort. If structured parsing fails, OpenAgent keeps a text fallback and records the parse error for diagnostics.

## ContextPackBuilder

`ContextPackBuilder` is the trace-first path toward one model-message assembly pipeline. It records:

- which context items were considered;
- why they were included;
- what priority they had;
- what was dropped or degraded under budget pressure.

This makes context behavior debuggable before changing the exact model input semantics.

## P0 Persistence

When `session_store` is enabled, each step now writes a metadata-only context pack snapshot under the run directory:

```text
.openagent/sessions/{session_id}/runs/{run_id}/context/context-pack-step-0001.json
```

The snapshot records item kind, source, priority, token estimate, included/drop status, and budget stage. It intentionally avoids duplicating full prompt text, file content, and tool output. The run ledger also records `context.pack_snapshot.saved`, and `Session.metadata["last_context_pack_snapshot"]` points to the latest snapshot.

Use `resume_session(...)` to restore a persisted `Session`, and `load_latest_context_pack_snapshot(...)` to inspect the latest context pack evidence for that session.

See [`context-persistence-p0.md`](context-persistence-p0.md) for the P0 requirement and acceptance checklist.

## P1 Assets

P1 promotes context sources into resumable assets:

- instruction snapshots record loaded instruction files, scopes, byte counts, truncation state, and content hashes;
- file context snapshots record read files, hashes, mtimes, previews, and change status;
- `session-memory.md` provides a lightweight continuation packet for long-running sessions;
- `resume_session(...)` validates the latest instruction/file assets and reports whether they are unchanged, changed, missing, or unchecked.

The assets live beside the P0 context pack snapshots:

```text
.openagent/sessions/{session_id}/
  session-memory.md
  runs/{run_id}/context/context-assets-step-0001.json
```

See [`context-persistence-p1.md`](context-persistence-p1.md) for the P1 requirement and acceptance checklist.

## Design Rule

Add new context sources as explicit `ContextItem`s with priority, source, stability, and metadata. Avoid hiding important state in ad hoc prompt text.
