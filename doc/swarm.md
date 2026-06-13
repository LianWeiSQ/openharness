# Swarm Function Kernel

OpenAgent is starting to grow a separate swarm/function kernel. The kernel lives in `src/swarm/` and intentionally has no `openagent` imports.

## Current Scope

Implemented in this slice:

- agent-agnostic protocol types: `AgentRunner`, `AgentSpec`, `AgentResult`, `AgentDescriptor`, `RunContext`, limits, usage, and fanout budget;
- `FunctionRunner`, which adapts a normal Python callable into a runner endpoint;
- YAML config loading for runners, tasks, limits, and fanout budget;
- `SwarmRuntime`, a minimal supervisor that dispatches one task to one or multiple runners;
- `OpenAgentRunner`, an adapter in `openagent.integrations.swarm` that lets OpenAgent act as one runner endpoint;
- local swarm trace lineage for run, task, runner, and runner-event spans;
- tests proving function dispatch, OpenAgent dispatch, multi-runner aggregation, trace lineage, failure capture, contract validation, and the OpenAgent boundary.

Not implemented yet:

- HTTP, subprocess, or A2A runners;
- persistent team state;
- write-capable worker isolation;
- Langfuse export for swarm spans.

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

This is intentionally local and SDK-free. The next stage can map these events into Langfuse spans without changing runner behavior.

## Design Boundary

The kernel is meant to standardize how a coordinator reaches agents. OpenAgent is one runner through an adapter, but the kernel must remain usable without OpenAgent installed.

Every task sent to a runner must carry the anti-drift contract:

- `objective`
- `context`
- `boundaries`
- `output_schema`

Missing fields fail the runner result instead of silently executing a vague task.

## Next Slices

1. Add Langfuse export for swarm trace events.
2. Add subprocess and HTTP runners for non-OpenAgent agents.
3. Add file/worktree isolation before write-capable workers.
