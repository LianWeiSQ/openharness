# Swarm Function Kernel

OpenAgent is starting to grow a separate swarm/function kernel. The kernel lives in `src/swarm/` and intentionally has no `openagent` imports.

## P0 Scope

Implemented in this slice:

- agent-agnostic protocol types: `AgentRunner`, `AgentSpec`, `AgentResult`, `AgentDescriptor`, `RunContext`, limits, usage, and fanout budget;
- `FunctionRunner`, which adapts a normal Python callable into a runner endpoint;
- YAML config loading for runners, tasks, limits, and fanout budget;
- `SwarmRuntime`, a minimal supervisor that dispatches one task to one or multiple runners;
- tests proving function dispatch, multi-runner aggregation, failure capture, contract validation, and the OpenAgent boundary.

Not implemented yet:

- OpenAgent runner adapter;
- HTTP, subprocess, or A2A runners;
- persistent team state;
- write-capable worker isolation;
- Langfuse trace lineage for swarm spans.

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

## Design Boundary

The kernel is meant to standardize how a coordinator reaches agents. OpenAgent can become one runner through an adapter later, but the kernel must remain usable without OpenAgent installed.

Every task sent to a runner must carry the anti-drift contract:

- `objective`
- `context`
- `boundaries`
- `output_schema`

Missing fields fail the runner result instead of silently executing a vague task.

## Next Slices

1. Add `OpenAgentRunner` as an adapter outside the kernel boundary.
2. Add swarm trace lineage and Langfuse span mapping.
3. Add subprocess and HTTP runners for non-OpenAgent agents.
4. Add file/worktree isolation before write-capable workers.
