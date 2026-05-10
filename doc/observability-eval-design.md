# P0 Observability And Eval Design

This document describes the first production-oriented observability and eval loop for OpenAgent.

## Goals

P0 makes OpenAgent runs traceable, replayable, and measurable without introducing an external telemetry service.

The first version focuses on:

- structured run / step / model / tool / context events
- safe metadata storage on `Session.metadata["observability"]`
- optional JSONL traces
- deterministic eval cases
- local eval reports and trace summaries

## Observability Model

OpenAgent records a trace per `AgentLoop.run()` call.

The trace root is:

```text
Session.metadata["observability"]
```

It contains:

- `trace`: trace metadata such as `trace_id`, `run_id`, `session_id`, agent, model, provider, and workspace
- `events`: recent structured events, kept as a ring buffer
- `event_count`: total number of emitted events
- `jsonl_path`: trace file path when JSONL is enabled

The event shape is defined by `ObservationEvent`:

```text
event_id
trace_id
run_id
session_id
span_id
parent_span_id
name
kind
timestamp_ms
duration_ms
status
attributes
```

Default configuration:

```python
options={
    "observability": {
        "enabled": True,
        "keep_events": True,
        "jsonl": False,
        "jsonl_dir": ".openagent/observability",
        "max_events": 500,
        "input_preview_chars": 2048,
        "include_traceback": False,
    }
}
```

`observability` is a runtime-only option and is not forwarded to model providers.

## Recorded Events

The first implementation records:

- `run.started`
- `run.finished`
- `run.failed`
- `step.started`
- `step.finished`
- `model.call.started`
- `model.call.finished`
- `model.call.failed`
- `model.usage`
- `tool.call.started`
- `tool.call.finished`
- `tool.call.failed`
- `context.budget_checked`
- `context.compaction.started`
- `context.compaction.finished`
- `patch.detected`
- `question.requested`
- `permission.denied`
- `doom_loop.detected`
- `error`

The events intentionally prefer low-cardinality, aggregate-friendly fields:

- `agent_name`
- `model_id`
- `provider_id`
- `step_index`
- `attempt_index`
- `tool_name`
- `finish_reason`
- `error_kind`
- `fallback_stage`
- `projection`
- `input_tokens`
- `output_tokens`
- `cost`
- `duration_ms`

## Safety

Observability must not become a hidden data leak.

The sanitizer redacts fields whose keys include:

```text
token
secret
password
api_key
authorization
cookie
```

Token usage metric fields such as `input_tokens` and `output_tokens` are preserved because they are operational metrics, not credentials.

Tool input is stored as a redacted preview. Tool output is not stored in full; events record output size, truncation status, and output path when available.

## Eval Loop

The local eval runner lives under `openagent.core.eval`.

Eval cases can be JSON or YAML:

```yaml
id: context_compaction_001
input: "继续完成整改，保持之前不做 LSP 的决定"
workspace: fixtures/context_project
history: fixtures/history/long_context.json
expected:
  must_remember:
    - "短期不做 LSP"
  files_changed: []
scoring:
  require_no_error: true
  require_final_answer_contains:
    - "上下文"
```

The first scorer is deterministic. It supports:

- final answer contains / not contains
- file exists
- file contains
- changed files allowlist
- no error event
- max steps
- max cost
- required tool called
- forbidden tool not called
- context decision remembered

Eval reports are written to:

```text
.openagent/eval-runs/<run>/report.json
.openagent/eval-runs/<run>/summary.md
```

Each result includes:

```text
case_id
status
score
duration_ms
steps
tool_calls
input_tokens
output_tokens
cost
error_kind
failure_reasons
trace_path
```

## Replay

P0 replay is offline trace inspection, not deterministic model replay.

`openagent.core.eval.replay` can:

- load JSONL trace events
- summarize model calls, tool calls, context events, errors, tokens, and cost
- render a Markdown trace summary

Future work can add deterministic model replay by storing model stream fixtures and replaying `AgentLoop` without live model calls.
