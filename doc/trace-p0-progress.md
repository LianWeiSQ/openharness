# Trace P0 Progress

## Goal

Build a generic Agent Run Trace foundation under `openagent.core.trace` for all Agent Runtime paths. The trace layer is domain-agnostic: vertical product capability is added by Skill, MCP, Tool, OpenAPI, or Sandbox integrations, while runtime observation remains unified.

## Task Plan

1. Trace Schema: define generic run, step, model call, tool call, artifact, and error records.
2. Trace Recorder: write `trace.jsonl`, `summary.json`, `process.md`, and artifact directory per run.
3. Runtime Integration: connect recorder to AgentLoop observation events and model/tool spans.
4. Tool / Skill / MCP Integration: classify tool source consistently at the Tool Runtime wrapper layer.
5. CLI + Verification: provide trace inspection commands and run tests plus local model service smoke when available.

## Receipts

- 2026-06-09: Started P0. Confirmed existing OpenAgent has `ObservationRecorder`, `RuntimeLogger`, AgentLoop model/tool spans, Tool middleware, and MCP tools wrapped as ToolDefinitions. Chosen approach: build `openagent.core.trace` and sync existing observation events into standardized trace files instead of adding per-business-agent logging.
- 2026-06-09: Task 1 completed. Added `openagent.core.trace.schema` with `RunRecord`, `TraceEvent`, `StepRecord`, `ModelCallRecord`, `ToolCallRecord`, `ArtifactRecord`, `ErrorRecord`, and `TraceConfig`. The schema is domain-agnostic and treats Skill, MCP, OpenAPI, Sandbox, and local tools as tool sources.
- 2026-06-09: Task 2 completed. Added `AgentTraceRecorder` to write `.openagent/runs/{run_id}/trace.jsonl`, `summary.json`, `process.md`, and `artifacts/`. The recorder supports redaction, summary aggregation, event loading, run listing, and summary rendering.
- 2026-06-09: Task 3 completed. Integrated trace recording with `ObservationRecorder`, so AgentLoop `run/step/model/tool/error` events are mirrored into standard trace files without business-agent-specific logging.
- 2026-06-09: Task 4 completed. Added Tool Runtime metadata propagation and middleware classification for `local_tool`, `skill`, and `mcp` sources. MCP tool calls are identified through bridge metadata such as `backend=mcp`, `mcp_server`, and `mcp_original_tool_name`.
- 2026-06-09: Task 5 completed. Added `python -m openagent.core.trace` CLI with `list`, `show`, `summary`, and `events`. Added trace tests and verified with full OpenAgent unit tests.
- 2026-06-09: Runtime safety fix. Trace files initially caused workspace patch events when written outside `.openagent`; updated `SnapshotManager` to ignore `.git`, `.openagent`, and `__pycache__`, keeping runtime artifacts out of workspace diffs.
- 2026-06-09: Verification. `PYTHONPATH=openagent/src:openagent/src/tests python -m unittest discover -s openagent/src/tests -p 'test_*.py'` passed: 199 tests OK.
- 2026-06-09: Local model smoke. Common local model ports `11434` and `1234` were not listening. `127.0.0.1:8080` is an OpenAI-compatible gateway but requires a valid API key, so real gateway smoke is blocked by credentials. A temporary localhost OpenAI-compatible SSE service was started and called through `OpenAILanguageModel`; AgentLoop completed successfully, trace summary reported status `completed`, `model_call_count=1`, `total_input_tokens=11`, `total_output_tokens=3`, and the trace CLI returned code 0.
- 2026-06-09: P0 benchmark pass against local `references/cc-code` and `references/opencode`.
  - `cc-code` reference points: `sessionTracing.ts`, `perfettoTracing.ts`, `events.ts`, and `logs.ts`. P0-relevant takeaways: span lifecycle cleanup, monotonic event sequence, prompt/content redaction by default, duration-oriented trace events, and bounded trace growth. P0 already has sequence, duration, redaction, and bounded in-memory events; no Perfetto/OTel exporter was added because that is outside P0.
  - `opencode` reference points: `v2/session-event.ts`, `session/session.ts`, `http-recorder/test/record-replay.test.ts`, and benchmark submission schema. P0-relevant takeaways: explicit session event taxonomy, step token structure including reasoning/cache buckets, record-replay validation mindset, and benchmark result persistence. P0 added text stream events, reasoning/cache token summary placeholders, and a trace integrity checker; HTTP cassette replay and benchmark DB were deferred because they belong to later Eval/Benchmark phases.
- 2026-06-09: P0 optimization completed after benchmark pass.
  - Added `text.started`, `text.delta`, and `text.finished` trace events. These record text ids and character counts, not generated text content, to keep P0 useful without expanding content exposure.
  - Added summary fields for `total_reasoning_tokens`, `total_cache_read_tokens`, and `total_cache_write_tokens`.
  - Added `python -m openagent.core.trace check {run_id}` and `check_trace_run()` to validate required P0 closure: run start/terminal event, step start/finish, model call start/finish, contiguous sequence, and summary event count consistency.
  - Added tests for text event tracing and trace integrity checks.
- 2026-06-09: Verification after benchmark optimization. `PYTHONPATH=openagent/src:openagent/src/tests python -m unittest discover -s openagent/src/tests -p 'test_*.py'` passed: 201 tests OK.

## Current Status

P0 is implemented, benchmark-adjusted, and verified locally. The remaining work belongs to later phases:

- Add database-backed trace storage if runtime needs cross-session query.
- Record MCP `list_tools` lifecycle events directly from `RemoteMcpManager` when a runtime recorder is available.
- Add Eval and Cost regression reports on top of `summary.json` and `trace.jsonl`.
- Wire a real local gateway smoke once a valid local API key is provided in the environment.
- Add HTTP record/replay cassette support if model/tool integration tests need deterministic external traffic.
