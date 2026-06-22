# OpenAgent Maintenance Report

Last updated: 2026-06-11

This file records the issues and follow-up work discovered while building the OpenAgent trace, eval, cost, runtime-warning, and external-observability slices. Use it as the human-readable maintenance entry. The machine-readable queue lives in `tasks.json`, and session notes live in `progress.md`.

## Current State

Recent core checkpoints:

- `44861ee`: added P0 trace foundation.
- `782907f`: added trace-backed eval regression.
- `4886a8f`: removed LangSmith / OpenTelemetry exporter.
- `43bc3ca`, `53d058d`, `88ff68a`, `e2f933d`: added Langfuse optional dependency, trace export, eval score export, and docs.
- `cd4a9fa`, `fd2458c`, `ba70532`, `200faf0`: added eval budget gates, runtime-warning trace events, realtime warning outputs, CI gate, and Langfuse warning metrics.

Current local uncommitted changes:

- `doc/operations.md`: adds step budget guidance based on a real `gpt-5.5` Langfuse smoke task.
- `doc/roadmap.md`: adds adaptive step budgeting as a roadmap item.
- `src/openagent/core/message_materializer.py`: adds `trace` and `runtime_warnings` to `RUNTIME_OPTION_KEYS`.
- `src/tests/test_context_budget.py`: adds coverage that `trace` and `runtime_warnings` runtime options are stripped from provider-facing model options.

Interpretation: these are active local maintenance changes. Do not revert them casually.

## Maintenance Principles

- Local trace artifacts remain the source of truth. External sinks such as Langfuse are optional views.
- Default exports must stay metadata-only. Prompt content, model output, tool input/output, and workspace paths require explicit opt-in.
- Eval reports must remain useful without Langfuse credentials.
- Runtime warnings should become actionable gates, not noisy logs.
- Avoid reintroducing LangSmith unless there is a new explicit decision.

## Known Issues And Follow-Ups

### OA-001 Runtime Options Can Leak Into Provider Options

Status: in progress

Evidence:

- Current local diff adds `trace` and `runtime_warnings` to `RUNTIME_OPTION_KEYS`.
- Without this, internal runtime config can be forwarded into OpenAI-compatible provider options.

Impact:

- Provider requests may include internal OpenAgent-only options.
- Future runtime config keys can repeat this class of bug.

Next action:

- Finish and commit the local `message_materializer.py` / `test_context_budget.py` change.
- Add a small rule: every new `AgentConfig.options` runtime namespace must be listed in `RUNTIME_OPTION_KEYS` or explicitly documented as provider-facing.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_context_budget.py
PYTHONPATH=src:src/tests python -m unittest discover -s src/tests -p "test_*.py"
```

### OA-002 ContextPackBuilder Is Not Yet The Single Message Assembly Path

Status: pending

Evidence:

- `doc/roadmap.md` says `ContextPackBuilder` is trace-first, not yet the only model-message assembly path.

Impact:

- Context budget, context pack diagnostics, and runtime warnings can diverge across paths.
- Future agent features may patch the old path instead of the canonical path.

Next action:

- Map every model-message assembly path.
- Move remaining paths behind `ContextPackBuilder`.
- Keep provider-specific materialization isolated after context assembly.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_context_budget.py src/tests/test_loop.py
```

### OA-003 Persistent Session Storage Is Not Wired Into The Main Loop

Status: pending

Evidence:

- `doc/roadmap.md` lists persistent session storage as incomplete.

Impact:

- Pause/resume, run recovery, and cross-session trace/eval correlation remain limited.

Next action:

- Wire storage into run start, step finish, pause, resume, and compaction.
- Add storage-backed tests for interrupted runs.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_loop.py
```

### OA-004 Real Local Model Gateway Smoke Is Blocked By Credentials

Status: blocked

Evidence:

- P0 smoke found `127.0.0.1:8080` is OpenAI-compatible but requires a valid API key.
- Temporary localhost SSE smoke passed, but real gateway smoke did not run.

Impact:

- Trace/eval model-call path is tested with mocks and temporary local service, not the actual local gateway.

Next action:

- Provide a valid local gateway API key through environment variables.
- Add a documented smoke command that runs one AgentLoop call and checks trace summary.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_trace.py
```

### OA-005 MCP `list_tools` Lifecycle Is Not Directly Traced

Status: pending

Evidence:

- P0 traces MCP tool calls through tool metadata.
- MCP discovery/list lifecycle is not directly recorded from `RemoteMcpManager`.

Impact:

- Tool-call traces show MCP usage after discovery, but do not explain discovery latency, auth failures, or missing-tool causes.

Next action:

- Add observation events around MCP server connect, `tools/list`, refresh, failure, and cache hit/miss.
- Preserve credential redaction.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_trace.py
```

### OA-006 Artifact Tracking Is Still Partial

Status: pending

Evidence:

- P0 summary increments artifact count for `artifact.created` and some `output_path` tool metadata.
- There is no fully wired artifact manager for all generated artifacts.

Impact:

- Trace summaries can undercount or inconsistently classify artifacts.

Next action:

- Define artifact kinds and ownership.
- Record artifact creation from file tools, eval reports, trace exports, and benchmark outputs consistently.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_trace.py src/tests/test_eval_runner.py
```

### OA-007 Eval Suite Runner Needs A Stable CLI

Status: pending

Evidence:

- `run_eval_files(...)` exists.
- `openagent.core.eval.ci_gate` exists.
- There is not yet a stable CLI that runs suites, writes reports, passes baselines, and applies thresholds.

Impact:

- Repeated local and CI eval workflows require Python glue.

Next action:

- Add `python -m openagent.core.eval run ...`.
- Support cases glob, output dir, baseline report, regression thresholds, and CI gate options.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_eval_runner.py
```

### OA-008 HTTP / Model / Tool Record-Replay Is Not Implemented

Status: pending

Evidence:

- P0 benchmark notes deferred HTTP cassette replay.

Impact:

- External model, MCP, and HTTP tool tests are harder to make deterministic.

Next action:

- Add a cassette layer only for integration tests that require external traffic.
- Keep core runtime independent from cassette format.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest discover -s src/tests -p "test_*.py"
```

### OA-009 Trace And Eval Storage Are File-Based Only

Status: pending

Evidence:

- Trace writes `.openagent/runs/{run_id}` or eval output `runs/{run_id}`.
- No database-backed query layer exists.

Impact:

- Cross-run search, dashboards, long-term history, and team review rely on filesystem layout or external sinks.

Next action:

- Define a read-only index first: run id, case id, status, model/tool counts, cost, latency, warning count.
- Add DB storage only after query requirements are clear.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_trace.py src/tests/test_eval_runner.py
```

### OA-010 Langfuse Real Smoke Still Requires Credentials

Status: blocked

Evidence:

- Langfuse exporter and eval score export have fake-client tests.
- Real Langfuse upload smoke requires `LANGFUSE_PUBLIC_KEY`, `LANGFUSE_SECRET_KEY`, and `LANGFUSE_BASE_URL`.

Impact:

- API drift or cloud-side behavior changes will not be caught locally.

Next action:

- Add a manual smoke runbook and optional env-gated test.
- Keep fake-client unit tests as the default CI path.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_trace.py src/tests/test_eval_runner.py
```

### OA-011 LangSmith Was Removed By Decision

Status: done / guardrail

Evidence:

- `4886a8f` removes the LangSmith / OpenTelemetry exporter, optional dependency, docs, and tests.
- Current repository scan has no LangSmith or OTel references.

Impact:

- This reduces external-observability scope and keeps Langfuse as the current optional sink.

Maintenance rule:

- Do not reintroduce LangSmith without a new explicit decision and a separate design note.

Verify:

```bash
rg -n "LangSmith|langsmith|LANGSMITH|opentelemetry|OTLP|otel" pyproject.toml doc src/openagent src/tests || true
```

### OA-012 Runtime Warning Thresholds Need Operational Calibration

Status: pending

Evidence:

- Runtime warnings and CI gates exist.
- Threshold defaults are still generic.

Impact:

- Warnings can be too noisy or too weak to protect cost/latency budgets.

Next action:

- Collect several eval runs.
- Tune context usage ratio, critical ratio, max step tokens, and max step cost.
- Document recommended thresholds for local, CI, and benchmark modes.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_eval_runner.py
```

### OA-013 Optional Dependencies Need Further Splitting

Status: pending

Evidence:

- `doc/roadmap.md` lists splitting optional dependencies for MCP, sandbox, and benchmark integrations.

Impact:

- Installing the package can pull more capabilities than a user needs.

Next action:

- Review `pyproject.toml` extras.
- Split extras by runtime path: MCP, sandbox, benchmark, langfuse.

Verify:

```bash
python -m py_compile src/openagent/core/trace/exporter.py
```

### OA-014 Terminal-Bench And Harbor Reports Need Reproducible Publishing

Status: pending

Evidence:

- Adapters exist.
- Roadmap still calls for reproducible benchmark reports.

Impact:

- Benchmark claims are hard to compare across commits.

Next action:

- Define benchmark report schema.
- Store command, environment, model, cost, latency, pass/fail, and trace path.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_terminal_bench_adapter.py src/tests/test_harbor_adapter.py
```

### OA-015 CLI / Web Console Should Stay Separate From Core

Status: pending

Evidence:

- Roadmap says CLI and Web Console are outside the public core.

Impact:

- Demo/UI concerns can pollute runtime package boundaries if rebuilt in core.

Next action:

- Keep core runtime headless.
- Build demo package with sanitized config templates when needed.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest discover -s src/tests -p "test_*.py"
```

### OA-016 Data Boundary Tests Need To Stay Strong

Status: pending

Evidence:

- Trace and Langfuse exports default to metadata-only.
- Redaction is implemented for common secret-bearing keys.

Impact:

- Future exporters or content opt-in paths may leak prompt/tool content or credentials.

Next action:

- Add exporter-level tests for secret redaction, content-disabled mode, and content-enabled opt-in.
- Require any new exporter to document data boundary behavior.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_trace.py
```

### OA-017 Step Budgeting Is Static And Can Block Required Closeout

Status: in progress

Evidence:

- Current local `doc/operations.md` and `doc/roadmap.md` changes record step-budget guidance.
- A real `gpt-5.5` Langfuse smoke task needed separate runs because complex investigation and code edits exhausted useful tool-enabled steps before final report writing.
- `max_steps` limits model-loop turns, and the final step is text-only because tools are disabled when the loop reaches the max step boundary.

Impact:

- Complex tasks can spend too many steps on inspection/confirmation.
- Required closeout artifacts, reports, or final verification can be pushed past the final tool-enabled step.
- Users see a final answer that explains what could not be written, which should be treated as a step-budget miss.

Next action:

- Keep the current documentation update.
- Add runtime warnings for remaining-step pressure if not already present.
- Consider adaptive defaults by task type and closeout protection for required artifacts.
- Detect read-only loops that consume many steps without making progress.

Verify:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_loop.py src/tests/test_eval_runner.py
```

## Standard Verification Commands

Fast local smoke:

```bash
bash init.sh
```

Trace and eval:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_trace.py src/tests/test_eval_runner.py
```

Full regression:

```bash
PYTHONPATH=src:src/tests python -m unittest discover -s src/tests -p "test_*.py"
```

Residual-risk scan:

```bash
rg -n "TODO|FIXME|LangSmith|langsmith|opentelemetry|OTLP|otel" pyproject.toml doc src/openagent src/tests
```
