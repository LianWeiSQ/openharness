# Swarm Function Kernel

OpenAgent is starting to grow a separate swarm/function kernel. The kernel lives in `src/swarm/` and intentionally has no `openagent` imports.

## Current Scope

Implemented in this slice:

- agent-agnostic protocol types: `AgentRunner`, `AgentSpec`, `AgentResult`, `AgentDescriptor`, `RunContext`, limits, usage, and fanout budget;
- `FunctionRunner`, which adapts a normal Python callable into a runner endpoint;
- YAML config loading for runners, tasks, limits, and fanout budget;
- `SwarmRuntime`, a minimal supervisor that dispatches one task to one or multiple runners;
- `OpenAgentRunner`, an adapter in `openagent.integrations.swarm` that lets OpenAgent act as one runner endpoint;
- `SubprocessRunner`, a CLI-agent adapter that talks JSON over stdin/stdout;
- `HttpRunner`, a remote-agent adapter that talks the same JSON protocol over HTTP;
- `A2ARunner`, an HTTP+JSON adapter for Agent2Agent-compatible remote agents;
- opt-in worker workspace isolation for future write-capable workers;
- merge-back conflict review for isolated worker outputs;
- coordinator-level merge approval policy for deciding whether a merge plan can be applied;
- optional file-backed persistent swarm run state;
- resumable coordinator policy for reusing completed runner results;
- local swarm trace lineage for run, task, runner, and runner-event spans;
- optional Langfuse export for swarm trace events;
- tests proving function dispatch, OpenAgent dispatch, subprocess dispatch, HTTP dispatch, multi-runner aggregation, trace lineage, Langfuse export mapping, failure capture, contract validation, and the OpenAgent boundary.

Not implemented yet:

- streaming A2A support.

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

A2A runners call standard Agent2Agent HTTP+JSON endpoints. The runner uses the `POST /message:send` binding with `application/a2a+json` request bodies and an `A2A-Version` header.

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

The exporter maps the local tree into observations:

```text
swarm.run        -> agent observation
  swarm.task     -> span observation
    swarm.runner -> span observation
      runner.*   -> instant span observations
```

By default the exporter is metadata-only. It sends run, task, runner, status, duration, usage, cost, and transport metadata. It does not export task context, objectives, prompts, model outputs, tool inputs, runner messages, or summaries unless `include_content=True` is explicitly set.

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

1. Add streaming A2A support for long-running remote agents.
2. Add resumable team adapters for multi-session worker handoff.
3. Add a coordinator workflow that combines resume, merge approval, and optional apply into one run receipt.
