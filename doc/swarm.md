# Swarm Function Kernel

OpenAgent is starting to grow a separate swarm/function kernel. The kernel lives in `src/swarm/` and intentionally has no `openagent` imports.

## Current Scope

Implemented in this slice:

- agent-agnostic protocol types: `AgentRunner`, `AgentSpec`, `AgentResult`, `AgentDescriptor`, `RunContext`, limits, usage, and fanout budget;
- `FunctionRunner`, which adapts a normal Python callable into a runner endpoint;
- YAML config loading for runners, tasks, limits, and fanout budget;
- `SwarmRuntime`, a minimal supervisor that dispatches one task to one or multiple runners;
- `OpenAgentRunner`, an adapter in `openagent.integrations.swarm` that lets OpenAgent act as one runner endpoint;
- `build_openagent_registry(...)`, a config-driven OpenAgent runner builder for mixed registries;
- `SubprocessRunner`, a CLI-agent adapter that talks JSON over stdin/stdout;
- `HttpRunner`, a remote-agent adapter that talks the same JSON protocol over HTTP;
- `A2ARunner`, an HTTP+JSON adapter for Agent2Agent-compatible remote agents, including SSE streaming and task subscription reconnect;
- opt-in worker workspace isolation for future write-capable workers;
- merge-back conflict review for isolated worker outputs;
- coordinator-level merge approval policy for deciding whether a merge plan can be applied;
- optional file-backed persistent swarm run state;
- resumable coordinator policy for reusing completed runner results;
- team handoff manifests for carrying multi-runner progress across sessions;
- combined coordinator workflow for run, handoff, merge approval, optional apply receipts, and observability-ready run summaries;
- CLI entrypoint for running YAML configs against subprocess, HTTP, and A2A runners;
- local swarm trace lineage for run, task, runner, and runner-event spans;
- optional Langfuse export for swarm trace events;
- tests proving function dispatch, OpenAgent dispatch, subprocess dispatch, HTTP dispatch, A2A dispatch, CLI YAML execution, multi-runner aggregation, trace lineage, Langfuse export mapping, failure capture, contract validation, and the OpenAgent boundary.

## Configuration Shape

```yaml
fanout_budget:
  max_concurrent: 2
  max_total_workers: 4

runners:
  researcher:
    kind: function
    roles: [research]
    handler: research_fn

tasks:
  compare:
    role: research
    objective: Compare two implementations.
    context: Repo paths are provided in inputs.
    boundaries: Read-only. Return evidence and open questions.
    output_schema:
      type: object
      required: [summary]
    runner_ids: [researcher]
```

The YAML file does not import functions by itself. Code binds function handlers explicitly:

```python
from swarm import SwarmRuntime, build_function_registry, load_swarm_config

config = load_swarm_config("swarm.yaml")
registry = build_function_registry(config, {"research_fn": research_fn})
result = await SwarmRuntime(registry=registry, fanout_budget=config.fanout_budget).run_task(config.task("compare"))
```

## CLI Runner

YAML configs that use external runner kinds can be run directly:

```bash
openagent-swarm run swarm.yaml --task compare --run-id compare-demo --pretty
```

The module form works without installing console scripts:

```bash
PYTHONPATH=src python -m swarm.cli run swarm.yaml --task compare --run-id compare-demo --pretty
```

By default the config-only CLI supports runner kinds that do not require in-process Python handler binding:

- `subprocess`
- `http`
- `a2a`

It intentionally does not auto-import `function` handlers. Function runners remain library-first because code must explicitly bind handler names to Python callables.

`openagent` runners are opt-in from the CLI. This keeps `src/swarm/` independent from OpenAgent while still allowing an installed OpenAgent package to bind YAML-configured workers:

```bash
OPENAI_API_KEY=... \
OPENAI_BASE_URL=http://localhost:8080 \
OPENAI_MODEL=gpt-5.5 \
OPENAI_WIRE_API=responses \
openagent-swarm run swarm.yaml \
  --task compare \
  --enable-openagent \
  --workspace . \
  --run-id compare-openagent \
  --pretty
```

OpenAgent CLI binding uses `OPENAGENT_SWARM_MODEL`, `OPENAI_MODEL`, `OPENAGENT_SWARM_CONTEXT_WINDOW`, `OPENAI_CONTEXT_WINDOW`, `OPENAGENT_SWARM_MAX_OUTPUT`, and `OPENAI_MAX_OUTPUT` for model metadata. `--model`, `--wire-api`, `--context-window`, and `--max-output` can override those values for one run.

Example subprocess YAML:

```yaml
runners:
  external_worker:
    kind: subprocess
    roles: [worker]
    metadata:
      command: ["python", "worker_agent.py"]

tasks:
  compare:
    role: worker
    objective: Compare two files.
    context: Paths are provided in inputs.
    boundaries: Read-only. Return JSON summary and evidence.
    output_schema:
      type: object
    runner_ids: [external_worker]
    inputs:
      paths: ["a.py", "b.py"]
```

Optional persistence:

```bash
openagent-swarm run swarm.yaml \
  --task compare \
  --run-id compare-demo \
  --state-dir .swarm/state \
  --handoff-dir .swarm/handoff \
  --pretty
```

The command writes JSON to stdout. The compact output includes:

- `run_id`
- `task_id`
- `status`
- `summary`
- `usage`
- `results`
- `warnings`
- `trace_event_count`
- optional `state_dir`
- optional coordinator `receipt`

When `--state-dir` is set, the full run state, runner results, and trace JSONL are written under `<state-dir>/<run-id>/`. When `--handoff-dir` is set, the coordinator also writes `<handoff-dir>/<run-id>/team-handoff.json` and `<handoff-dir>/<run-id>/coordinator-receipt.json`.

## Inspection UI And API

Persisted swarm runs can be inspected in a local browser view and through JSON endpoints:

```bash
openagent-swarm inspect \
  --state-dir .swarm/state \
  --handoff-dir .swarm/handoff \
  --host 127.0.0.1 \
  --port 8765
```

The module form is also available:

```bash
PYTHONPATH=src python -m swarm.cli inspect --state-dir .swarm/state --handoff-dir .swarm/handoff
```

Endpoints:

- `GET /`
- `GET /ui`
- `GET /health`
- `GET /runs`
- `GET /runs/{run_id}`
- `GET /runs/{run_id}/state`
- `GET /runs/{run_id}/handoff`
- `GET /runs/{run_id}/receipt`
- `GET /runs/{run_id}/trace`

`/` and `/ui` serve a small browser-facing inspection view that fetches the JSON API, renders persisted runs, and opens run details without a build step or external assets. `/runs` and `/runs/{run_id}` are compact by default and include diagnostics for malformed artifacts. Full trace events are only returned from `/runs/{run_id}/trace`.

## Worker Workspace Isolation

Workspace isolation is opt-in. It prepares a per-runner directory before dispatch and injects the path into:

- `AgentSpec.inputs["worker_workspace"]`
- `AgentSpec.metadata["worker_workspace"]`
- `RunContext.metadata["worker_workspace"]`
- `swarm.runner.finished` trace attributes

Task-level metadata defines defaults:

```yaml
tasks:
  edit:
    role: worker
    objective: Prepare a patch.
    context: Work from an isolated copy.
    boundaries: Only write inside worker_workspace. Do not modify source_root.
    output_schema:
      type: object
    metadata:
      isolation:
        enabled: true
        mode: copy
        source_root: "/path/to/repo"
        base_dir: "/tmp/openagent-swarm"
        exclude: [".git", ".venv", "__pycache__"]
```

Runner-level metadata can override the task default:

```yaml
runners:
  scratch_worker:
    kind: function
    roles: [worker]
    metadata:
      isolation:
        enabled: true
        mode: empty
```

Supported modes:

- `copy`: copy `source_root` into a fresh worker directory.
- `empty`: create a fresh empty worker directory.

This layer only prepares worker directories. Merge-back remains an explicit workflow, after review and conflict checks.

## Merge-Back Review

Merge-back is explicit and review-first. `SwarmRuntime.run_task(...)` does not write worker outputs back to the source workspace.

When isolation is enabled, each runner result preserves workspace metadata:

```python
result.results["alpha"].metadata["worker_workspace"]
result.results["alpha"].metadata["workspace_source_root"]
```

Build a merge plan from the run results:

```python
from swarm import apply_merge_plan, build_merge_plan

plan = build_merge_plan(result.results)

if not plan.has_conflicts:
    applied = apply_merge_plan(plan)
```

The merge planner reports:

- `added` files created by a worker;
- `modified` files changed from the source workspace;
- `deleted` files removed in a worker workspace;
- conflicts when multiple workers change the same relative path differently.

`apply_merge_plan(...)` skips conflicts by default. It applies only non-conflicting changes unless `include_conflicts=True` is explicitly set.

If a worker output needs merge-back review, keep isolation `cleanup` disabled until the merge plan has been built and reviewed.

## Merge Approval Policy

Merge approval is a coordinator decision layer above `build_merge_plan(...)`. It is read-only: it evaluates a plan and returns `approved`, `needs_review`, or `rejected`; it does not write files.

```python
from swarm import apply_merge_plan, build_merge_plan, evaluate_merge_plan

plan = build_merge_plan(result.results)
decision = evaluate_merge_plan(
    plan,
    {
        "auto_approve": True,
        "allow_deletions": False,
        "max_changed_files": 5,
        "max_total_bytes": 200_000,
        "protected_paths": ["pyproject.toml", ".github/**"],
    },
)

if decision.can_apply:
    applied = apply_merge_plan(plan)
```

Task metadata can carry the same policy:

```yaml
tasks:
  edit:
    role: worker
    objective: Prepare a patch.
    context: Work from isolated workers.
    boundaries: Only write inside worker_workspace.
    output_schema:
      type: object
    metadata:
      merge:
        approval:
          auto_approve: true
          allow_deletions: false
          max_changed_files: 5
          protected_paths: ["pyproject.toml", ".github/**"]
```

The policy blocks or escalates:

- conflicting edits to the same path;
- source-file deletions when `allow_deletions` is false;
- protected path matches;
- changed file count above `max_changed_files`;
- changed bytes above `max_total_bytes`.

Without `auto_approve`, a non-empty safe plan returns `needs_review`. An empty plan returns `approved`.

## Persistent State

Swarm run persistence is optional. A file store records a durable snapshot after `run_task(...)` completes.

```python
from swarm import FileSwarmStateStore, SwarmRuntime

store = FileSwarmStateStore(".swarm/runs")
runtime = SwarmRuntime(registry=registry, state_store=store)
result = await runtime.run_task(task, run_id="run-123")

saved = store.load_run("run-123")
```

Each run writes:

- `state.latest.json`: task id, run id, status, summary, usage, warnings, runner results, and trace events;
- `runner-results.json`: runner result payloads for fast coordinator review;
- `trace.jsonl`: local swarm trace events, one JSON object per line.

## Resume Policy

Resume is opt-in. By default, `SwarmRuntime.run_task(...)` reruns all selected runners even if a previous state file exists for the same `run_id`.

Enable resume at runtime:

```python
runtime = SwarmRuntime(
    registry=registry,
    state_store=store,
    resume_policy=True,
)
```

Or enable it from task metadata in YAML:

```yaml
tasks:
  compare:
    role: research
    objective: Compare two implementations.
    context: Repo paths are provided in inputs.
    boundaries: Read-only.
    output_schema:
      type: object
    runner_ids: [researcher, reviewer]
    metadata:
      resume:
        enabled: true
        reuse_statuses: [completed]
        strict_task_id: true
```

When resume is enabled, the coordinator loads `state.latest.json` for the requested `run_id`. If the saved `task_id` matches the current task, reusable runner results are restored before dispatch. By default only `completed` results are reused; missing, failed, partial, and cancelled results are rerun. Reused results are marked with:

```python
result.results["researcher"].metadata["resumed"]
result.results["researcher"].metadata["resumed_from_run_id"]
```

The run also records a `swarm.resume` trace event with reused and dispatched runner ids. The final state file is rewritten with the merged result set.

## Team Handoff

Team handoff manifests are optional coordinator receipts for long swarm jobs. They summarize a multi-runner result so a later session can see which runners are reusable and which still need work.

```python
from swarm import (
    FileTeamHandoffStore,
    build_team_handoff,
    task_for_team_handoff_resume,
)

handoff = build_team_handoff(
    task=task,
    result=result,
    run_id="run-123",
)

store = FileTeamHandoffStore(".swarm/runs")
store.save_handoff(handoff)

resume_task = task_for_team_handoff_resume(task=task, handoff=handoff)
resume_result = await runtime.run_task(resume_task, run_id=handoff.run_id)
```

Each `team-handoff.json` records:

- the task contract and original runner order;
- reusable runner ids, pending runner ids, and failed/partial/cancelled/missing runner ids;
- compact per-runner status, summary, evidence, and metadata;
- the resume reuse statuses that a follow-up run should use.

`task_for_team_handoff_resume(...)` injects `metadata.resume.enabled=true` and the handoff metadata. By default it keeps the original runner list so `SwarmRuntime` can reuse completed results and rerun pending ones with the same `run_id`. Pass `pending_only=True` when a coordinator wants a narrower follow-up task that targets only pending runners.

## Coordinator Workflow

`run_swarm_coordinator(...)` combines the durable pieces into one workflow:

1. optionally load an existing `team-handoff.json`;
2. derive a resume task for the same `run_id`;
3. run `SwarmRuntime.run_task(...)`;
4. save a fresh team handoff;
5. optionally build a merge plan;
6. evaluate merge approval;
7. optionally apply approved changes.

```python
from swarm import (
    FileTeamHandoffStore,
    SwarmCoordinatorOptions,
    run_swarm_coordinator,
)

result = await run_swarm_coordinator(
    runtime=runtime,
    task=task,
    team_handoff_store=FileTeamHandoffStore(".swarm/runs"),
    options=SwarmCoordinatorOptions(
        run_id="run-123",
        merge_enabled=True,
        apply_approved_merge=False,
    ),
)

receipt = result.receipt.as_dict()
```

The receipt records:

- schema version, run id, task id, task role, and aggregate run status;
- runner count and runner status counts;
- aggregate usage: input tokens, output tokens, total tokens, cost, steps, and latency;
- trace event count and trace error count;
- compact per-runner summaries with status, summary preview, evidence count, open question count, artifact count, confidence, usage, and safe metadata;
- whether a handoff was saved and whether pending runners remain;
- reusable and pending runner ids;
- merge decision, reason codes, change count, conflict count, and applied count;
- warnings and diagnostics.

The receipt is intentionally compact. It does not dump full trace events, prompts, task context, model output, tool output, or arbitrary runner metadata. Per-runner metadata is allowlisted for operational keys such as HTTP status, response format, return code, A2A task id/state, error kind, and workspace isolation fields.

By default the coordinator does not apply merge changes. Set `apply_approved_merge=True` to apply only when the merge approval decision is `approved`.

## Subprocess Runner

Subprocess runners let the kernel call external CLI agents without importing them.

```yaml
runners:
  external_researcher:
    kind: subprocess
    roles: [research]
    metadata:
      command: ["python", "agent.py"]
      cwd: "/path/to/agent"
      timeout_seconds: 30
```

Build a registry:

```python
from swarm import SwarmRuntime, build_subprocess_registry, load_swarm_config

config = load_swarm_config("swarm.yaml")
registry = build_subprocess_registry(config)
result = await SwarmRuntime(registry=registry).run_task(config.task("compare"))
```

The subprocess receives JSON on stdin:

```json
{
  "spec": {
    "role": "research",
    "objective": "...",
    "context": "...",
    "boundaries": "...",
    "output_schema": {},
    "inputs": {},
    "permissions": "READONLY"
  },
  "context": {
    "run_id": "swarm_...",
    "parent_span_id": "span_..."
  },
  "runner": {
    "id": "external_researcher",
    "kind": "subprocess"
  }
}
```

The subprocess may return an `AgentResult`-like JSON object on stdout:

```json
{
  "status": "completed",
  "summary": "result",
  "evidence": ["file.py:12"],
  "confidence": 0.8,
  "usage": {"input_tokens": 10, "output_tokens": 4, "cost": 0.01}
}
```

Plain stdout is accepted as a completed summary. Non-zero exit code, startup errors, and timeouts become failed `AgentResult` values.

## HTTP Runner

HTTP runners let the kernel call remote agents without importing their SDKs.

```yaml
runners:
  remote_researcher:
    kind: http
    roles: [research]
    metadata:
      url: "http://127.0.0.1:9000/run"
      method: POST
      timeout_seconds: 30
      headers:
        Authorization: "Bearer ${TOKEN}"
```

Build a registry:

```python
from swarm import SwarmRuntime, build_http_registry, load_swarm_config

config = load_swarm_config("swarm.yaml")
registry = build_http_registry(config)
result = await SwarmRuntime(registry=registry).run_task(config.task("compare"))
```

The HTTP endpoint receives the same JSON protocol as the subprocess runner in the request body:

```json
{
  "spec": {
    "role": "research",
    "objective": "...",
    "context": "...",
    "boundaries": "...",
    "output_schema": {},
    "inputs": {},
    "permissions": "READONLY"
  },
  "context": {
    "run_id": "swarm_...",
    "parent_span_id": "span_..."
  },
  "runner": {
    "id": "remote_researcher",
    "kind": "http"
  }
}
```

The endpoint may return an `AgentResult`-like JSON object:

```json
{
  "status": "completed",
  "summary": "remote result",
  "evidence": ["service:trace-id"],
  "confidence": 0.8,
  "usage": {"input_tokens": 10, "output_tokens": 4, "cost": 0.01}
}
```

Plain response bodies are accepted as completed summaries. HTTP status errors, request errors, and timeouts become failed `AgentResult` values with diagnostic metadata.

Request headers are sent as HTTP headers. They are not copied into the JSON `runner.metadata` payload, so bearer tokens and API keys are not echoed to the remote agent body.

## A2A Runner

A2A runners call standard Agent2Agent HTTP+JSON endpoints. By default, the runner uses the `POST /message:send` binding with `application/a2a+json` request bodies and an `A2A-Version` header.

```yaml
runners:
  a2a_researcher:
    kind: a2a
    roles: [research]
    metadata:
      url: "https://agent.example.com/a2a"
      version: "1.0"
      timeout_seconds: 30
      accepted_output_modes: ["text/plain"]
      headers:
        Authorization: "Bearer ${TOKEN}"
```

Build a registry:

```python
from swarm import SwarmRuntime, build_a2a_registry, load_swarm_config

config = load_swarm_config("swarm.yaml")
registry = build_a2a_registry(config)
result = await SwarmRuntime(registry=registry).run_task(config.task("compare"))
```

The runner converts `AgentSpec` into a `ROLE_USER` message with one text part containing the role, objective, context, boundaries, output schema, and inputs. Completed task artifact text becomes the `AgentResult.summary`. Input-required or working task states become `partial`; failed, rejected, or canceled states become `failed`.

Enable streaming with runner metadata:

```yaml
runners:
  a2a_streaming_researcher:
    kind: a2a
    roles: [research]
    metadata:
      url: "https://agent.example.com/a2a"
      streaming: true
      version: "1.0"
      timeout_seconds: 60
      accepted_output_modes: ["text/plain"]
```

With `streaming: true`, the runner normalizes the endpoint to `POST /message:stream`, sends `Accept: text/event-stream`, and parses Server-Sent Events whose `data:` payloads are A2A `StreamResponse` objects. The supported stream events are:

- `task`
- `message`
- `statusUpdate`
- `artifactUpdate`

Each stream item becomes an `a2a.stream.*` runner event in the local swarm trace. Streamed artifact text, status messages, and direct messages are folded into the final `AgentResult.summary`; terminal task states still control the final result status.

To reconnect to an already-started remote task, configure a task subscription:

```yaml
runners:
  a2a_reconnector:
    kind: a2a
    roles: [research]
    metadata:
      url: "https://agent.example.com/a2a"
      subscribe_task_id: "task-123"
      version: "1.0"
      timeout_seconds: 60
```

When `subscribe_task_id` is present, the runner normalizes the endpoint to `POST /tasks/{id}:subscribe`, sends `Accept: text/event-stream`, and parses the same A2A `StreamResponse` event shapes as `message:stream`. A task can override the runner default by setting `inputs.a2a_task_id`, `inputs.subscribe_task_id`, `metadata.a2a_task_id`, or `metadata.subscribe_task_id`.

The final result keeps the same `response_format: a2a-sse` metadata as streaming, plus:

- `a2a_task_id`
- `a2a_subscribed_task_id`
- `a2a_task_state`
- `a2a_stream_events`

## OpenAgent Adapter

OpenAgent integrates outside the kernel boundary:

```python
from openagent.integrations.swarm import OpenAgentRunner
from swarm import SwarmRuntime
from swarm.registry import RunnerRegistry

registry = RunnerRegistry()
registry.register(
    OpenAgentRunner(
        runner_id="openagent-reader",
        roles=["research"],
        model=language_model,
        model_metadata=model_metadata,
        workspace_root="/path/to/workspace",
        tools="readonly",
    )
)

result = await SwarmRuntime(registry=registry).run_task(task)
```

This dependency direction is deliberate:

```text
openagent.integrations.swarm -> swarm
src/swarm                  -X-> openagent
```

The adapter converts an `AgentSpec` into a bounded OpenAgent subagent instruction, runs an isolated `Session`, and returns a swarm `AgentResult` with summary, usage, session id, trace metadata, and tool-call count.

OpenAgent runners can also be built from YAML runner config:

```python
from openagent.integrations.swarm import build_openagent_registry
from swarm import build_a2a_registry, load_swarm_config
from swarm.registry import RunnerRegistry

config = load_swarm_config("swarm.yaml")
registry = RunnerRegistry()

for partial in (
    build_openagent_registry(
        config,
        model=language_model,
        model_metadata=model_metadata,
        workspace_root="/path/to/workspace",
    ),
    build_a2a_registry(config),
):
    for runner in partial.all():
        registry.register(runner)
```

The CLI can bind OpenAgent runners with `--enable-openagent`. Internally it dynamically loads `openagent.integrations.swarm`, builds an OpenAI-compatible language model from environment variables, and registers only the OpenAgent runners selected by the task. Without `--enable-openagent`, `kind: openagent` is rejected with a clear error so the swarm kernel remains safe to use without OpenAgent installed.

For advanced tests or embedded runtimes, use `build_openagent_registry_from_env(...)` with an injected `language_model` to avoid network access while preserving the same YAML binding behavior.

## Course Demo

The course demo is the easiest way to present swarm mode in a class or interview setting. It routes one teaching task to:

- `openagent_teacher`: an OpenAgent runner;
- `subprocess_checker`: a CLI-style external runner that validates the standard JSON payload path.

The default run is fully offline and uses a scripted OpenAgent model, so it is stable for slides, recordings, and CI:

```bash
PYTHONPATH=src python src/examples/swarm_course_demo.py
```

The same YAML can be switched to a real local OpenAI-compatible gateway:

```bash
export OPENAI_API_KEY=...
export OPENAI_BASE_URL=http://localhost:8080
export OPENAI_MODEL=gpt-5.5
export OPENAI_WIRE_API=responses

PYTHONPATH=src python src/examples/swarm_course_demo.py --real
```

The real mode delegates to:

```bash
openagent-swarm run src/examples/swarm_course_demo.yaml \
  --task lesson_walkthrough \
  --enable-openagent \
  --workspace . \
  --run-id swarm-course-demo-real \
  --pretty
```

This demo is intentionally smaller than the all-runners example. It is meant to teach the control flow: YAML task contract, OpenAgent binding, external runner binding, coordinator receipt, and trace count.

## Mixed OpenAgent + A2A Example

The public mixed-runner example demonstrates one task routed to two different agent endpoints from one YAML config:

- `openagent_researcher`: a local OpenAgent runner backed by a scripted model;
- `a2a_reviewer`: a local mock A2A HTTP+JSON endpoint.

Run it without model credentials or external services:

```bash
PYTHONPATH=src python src/examples/swarm_mixed_openagent_a2a.py
```

The example loads [swarm_mixed_openagent_a2a.yaml](../src/examples/swarm_mixed_openagent_a2a.yaml), patches the A2A URL to a local mock server, builds a mixed registry with `build_openagent_registry(...)` and `build_a2a_registry(...)`, runs `run_swarm_coordinator(...)`, and prints compact JSON with runner results, trace count, and coordinator receipt.

## Mixed All-Runners Example

The full mixed-runner example demonstrates one YAML-configured task routed to four integration paths:

- `openagent_researcher`: a local OpenAgent runner backed by a scripted model;
- `subprocess_checker`: a local CLI-style external agent that reads JSON from stdin;
- `http_planner`: a local mock HTTP JSON agent endpoint;
- `a2a_reviewer`: a local mock A2A HTTP+JSON endpoint.

Run it without model credentials or external services:

```bash
PYTHONPATH=src python src/examples/swarm_mixed_all_runners.py
```

The example loads [swarm_mixed_all_runners.yaml](../src/examples/swarm_mixed_all_runners.yaml), patches the HTTP/A2A URLs and subprocess command to local offline mocks, builds registries with `build_openagent_registry(...)`, `build_subprocess_registry(...)`, `build_http_registry(...)`, and `build_a2a_registry(...)`, then runs `run_swarm_coordinator(...)`. The printed JSON includes runner results, runner kinds, mock request counts, trace event count, and the compact coordinator receipt.

## Trace Lineage

Every `SwarmRuntime.run_task(...)` result includes `trace_events`.

The local trace tree is:

```text
swarm.run
  swarm.task
    swarm.runner    # one per runner dispatch
      runner.started / runner.finished
      openagent.* or function runner events
```

Each trace event carries:

- `trace_id`
- `run_id`
- `span_id`
- `parent_span_id`
- `name`
- `kind`
- `status`
- `runner_id`
- `task_id`
- `attributes`

The trace recorder is intentionally local and SDK-free. Langfuse export lives in a separate optional module and can be called after a run completes.

## Langfuse Export

Swarm trace export is optional. Local `SwarmRunResult.trace_events` remains the source of truth, and Langfuse is an external view.

```python
from swarm import export_swarm_trace_to_langfuse

exported = export_swarm_trace_to_langfuse(
    result.trace_events,
    options={
        "enabled": True,
        "keys_required": False,
        "environment": "local",
        "tags": ["openagent", "swarm"],
    },
)
```

When using the coordinator workflow, pass the compact receipt as run-level annotations:

```python
coordinator = await run_swarm_coordinator(...)
exported = export_swarm_trace_to_langfuse(
    coordinator.run_result.trace_events,
    options={"enabled": True, "keys_required": False},
    receipt=coordinator.receipt.as_dict(),
)
```

The exporter maps the local tree into observations:

```text
swarm.run        -> agent observation
  swarm.task     -> span observation
    swarm.runner -> span observation
      runner.*   -> instant span observations
```

By default the exporter is metadata-only. It sends run, task, runner, status, duration, usage, cost, and transport metadata. Receipt annotations add safe run-level metrics such as runner counts, status counts, usage totals, trace counts, handoff counts, and merge counts. It does not export task context, objectives, prompts, model outputs, tool inputs, runner messages, full runner summaries, handoff paths, or arbitrary diagnostics unless `include_content=True` is explicitly set for event attributes.

Export is non-fatal by default. Missing credentials or a missing optional Langfuse dependency produce diagnostics in `SwarmLangfuseExportResult`; `strict=True` raises instead.

## Design Boundary

The kernel is meant to standardize how a coordinator reaches agents. OpenAgent is one runner through an adapter, but the kernel must remain usable without OpenAgent installed.

Every task sent to a runner must carry the anti-drift contract:

- `objective`
- `context`
- `boundaries`
- `output_schema`

Missing fields fail the runner result instead of silently executing a vague task.

## Next Slices

1. Add richer mixed-runner examples for subprocess/http/a2a plus OpenAgent side by side.
2. Add a minimal browser page that consumes the inspection API.
