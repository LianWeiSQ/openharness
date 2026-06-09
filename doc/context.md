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

## Design Rule

Add new context sources as explicit `ContextItem`s with priority, source, stability, and metadata. Avoid hiding important state in ad hoc prompt text.
