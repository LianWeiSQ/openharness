# Eval P1 Progress

## Goal

Build a generic eval regression layer on top of the P0 Agent Run Trace. P1 stays domain-agnostic: product-specific behavior is expressed through cases, tools, skills, MCP servers, and scoring rules.

## Scope

1. Use P0 trace artifacts as the primary eval evidence.
2. Record trace integrity, event counts, model/tool counts, latency, token, cost, and tool-source metrics per case.
3. Add scoring rules for trace closure, required/forbidden trace events, model/tool call budgets, latency, and cost.
4. Add baseline regression comparison for prompt/model/tool changes.
5. Keep old observability JSONL replay compatible while making P0 `trace.jsonl` the preferred path.

## Receipts

- 2026-06-09: P1 started after P0 was pushed to GitHub at commit `44861ee`.
- 2026-06-09: Confirmed existing eval runner wrote `report.json` and `summary.md`, but result tracing still preferred old observability JSONL.
- 2026-06-09: Updated eval runner to enable P0 trace by default under the eval output directory and attach trace path, summary path, trace check result, model/tool/MCP/Skill/local counts, error count, artifact count, and latency to every `EvalResult`.
- 2026-06-09: Added trace-backed scoring rules: `require_trace_check`, `required_trace_events`, `forbidden_trace_events`, `required_tool_sources`, `max_model_calls`, `max_tool_calls`, `max_duration_ms`, `max_total_latency_ms`, and existing `max_cost`.
- 2026-06-09: Added baseline regression output through `baseline_report=...`, writing `regression.json` and `regression.md`.
- 2026-06-09: Updated replay summary to read both old observability events (`name`) and P0 trace events (`event`).

## Remaining Later Work

- Add a CLI wrapper for eval suites if repeated local runs need a stable command-line interface.
- Add HTTP record/replay cassettes only when model/tool integration tests need deterministic external traffic.
- Add database-backed eval history when cross-run querying becomes necessary.

## P1.5 LangSmith Export Receipt

- 2026-06-09: Added a minimal optional LangSmith OpenTelemetry exporter. It exports OpenAgent run, step, model, and tool spans from the P0 trace event stream while keeping local trace artifacts as the source of truth.
- 2026-06-09: Added exporter diagnostics under `Session.metadata["agent_trace"]["exporters"]` so upload failures do not break Agent execution by default.
- 2026-06-09: Kept content export disabled by default. The exporter sends identifiers, span kind, status, latency, model/tool metadata, MCP/Skill metadata, token counts, and cost unless `include_content` or `include_workspace` is explicitly enabled.
