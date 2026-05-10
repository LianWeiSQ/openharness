# Structured Work State Compaction Design

> Status: implemented in the OpenAgent core runtime.
> Scope: `src/openagent/core/context_state.py`, `context_messages.py`, and `loop/processor.py`.

## 1. Goal

OpenAgent previously compacted old conversation history into a single free-form summary. That saved tokens, but it made long-running sessions fragile because the model had to infer the real working state from prose.

Structured work state compaction upgrades that summary into a deterministic continuation packet. Every compaction should preserve:

- the current task and user intent
- completed work and important decisions
- files and artifacts that matter for the next step
- tool findings, failures, and evidence
- active todos, blockers, open questions, and likely next steps
- explicit risks and verification gaps

The model should receive a compact work state that is easy to scan, stable across providers, and safe to resume after context pressure.

## 2. Non-Goals

- This does not add LSP, embeddings, or semantic code retrieval.
- This does not replace the full message history stored in the session.
- This does not introduce a new memory system.
- This does not require all providers to support JSON mode.

## 3. Runtime Contract

The compaction record remains stored under:

```text
Session.metadata["context_compaction"]
```

The record is backward-compatible with the old format:

```json
{
  "summary": "rendered continuation state",
  "compacted_until": 12,
  "updated_at": 1760000000000
}
```

Structured compaction adds these fields:

```json
{
  "schema_version": 1,
  "format": "structured_work_state",
  "state": {
    "task": "...",
    "progress": ["..."],
    "decisions": ["..."],
    "files": [
      {"path": "src/app.py", "status": "read", "note": "contains the failing route"}
    ],
    "tool_findings": ["..."],
    "todos": ["..."],
    "open_questions": ["..."],
    "blockers": ["..."],
    "next_steps": ["..."],
    "risks": ["..."]
  },
  "summary": "[Structured work state]\n...",
  "compacted_until": 12,
  "updated_at": 1760000000000,
  "source": "model_json",
  "parse_error": null
}
```

`summary` stays as the canonical rendered string used by older callers. `state` is the structured payload for diagnostics, future context pack builders, and product UI.

## 4. Work State Schema

### Required Fields

- `task`: string
- `progress`: list of strings
- `decisions`: list of strings
- `files`: list of file objects
- `tool_findings`: list of strings
- `todos`: list of strings
- `open_questions`: list of strings
- `blockers`: list of strings
- `next_steps`: list of strings
- `risks`: list of strings

### File Object

```json
{
  "path": "relative/or/display/path",
  "status": "read|modified|created|deleted|mentioned|unknown",
  "note": "why this file matters"
}
```

The parser normalizes arbitrary provider output into this shape:

- non-string list items are converted to compact JSON strings
- empty strings are discarded
- file entries may be objects or strings
- unknown file statuses are normalized to `unknown`
- every list has bounded length and every item has bounded length

## 5. Prompting Strategy

The compaction model call uses a dedicated system prompt that asks for a JSON object only. It is provider-agnostic and does not rely on JSON mode.

The user prompt includes the current todo list as JSON when available. The model is instructed to preserve only actionable state, not chat etiquette or stale attempts.

## 6. Rendering Strategy

The rendered message injected back into the model starts with:

```text
[Structured work state]
```

Then it renders stable sections:

```text
Task:
...

Progress:
- ...

Decisions:
- ...
```

Empty sections are omitted except `Task`, which falls back to `(unspecified)`.

This keeps the compacted context readable even when a downstream model ignores metadata.

## 7. Failure Handling

Compaction must not fail simply because a provider returns markdown fences, explanatory text, or slightly malformed structure.

The parser supports:

- raw JSON objects
- fenced JSON blocks
- JSON embedded in surrounding prose
- legacy free-form summaries

If no usable content is produced, compaction fails and the existing budget fallback path handles the error. If a non-empty free-form summary is produced, it is wrapped as a structured fallback state with `source = "legacy_text_fallback"` and `parse_error` metadata.

## 8. Compatibility

`get_context_compaction()` accepts both old and new records. It returns a rendered summary either way.

Old metadata:

```json
{"summary": "Goal: continue", "compacted_until": 2}
```

still renders as a compacted context message. New metadata additionally exposes `state`, `format`, `schema_version`, and parser diagnostics.

## 9. Production Invariants

- A compaction record is invalid if `compacted_until` is outside the session message range.
- Rendered content is deterministic.
- Structured fields are bounded to avoid moving overflow from history into metadata.
- Prompt output parsing is best-effort but never silently stores an empty state.
- The old `summary` field remains present for compatibility.
- Tests cover JSON parsing, fenced JSON parsing, legacy fallback, message injection, and loop integration.

## 10. Future Extensions

This structure is intentionally close to the future `ContextPackBuilder` shape. Later work can rank `state.files`, `state.tool_findings`, and `state.todos` independently instead of treating the whole compaction as a single message.
