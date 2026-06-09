# Architecture

OpenAgent is a Python runtime for tool-using agents. The core is small by design: a session, an agent loop, a model provider, a tool registry, and runtime adapters for where tools execute.

```text
User task
  -> AgentLoop
  -> Context/materialized messages
  -> LanguageModel.stream(...)
  -> tool calls
  -> PermissionManager
  -> ToolkitAdapter
  -> WorkspaceRuntime
  -> stream events / trace / patches
```

## Core Modules

| Area | Path | Responsibility |
| --- | --- | --- |
| Agent loop | `src/openagent/core/loop/` | Multi-step run loop, model calls, tool execution, patch events |
| Agents | `src/openagent/core/agent/` | `UniversalAgent`, `PlanAgent`, `ExploreAgent` and prompt resolution |
| Session | `src/openagent/core/session/` | Messages, todos, metadata, pause/resume state |
| Providers | `src/openagent/core/provider/` | OpenAI-compatible and DashScope streaming adapters |
| Tools | `src/openagent/core/tool/` | Built-in tools, plugin registration, middleware |
| MCP | `src/openagent/core/mcp/` | Remote MCP config, discovery, bridge, runtime calls |
| Execution | `src/openagent/core/execution/` | Local and optional remote sandbox workspace runtime |
| Context | `src/openagent/core/context_*` | Budgeting, compaction, file context, context pack traces |
| Integrations | `src/openagent/integrations/` | Terminal-Bench and Harbor adapters |

## Tool Flow

The model receives tool schemas. If it emits a tool call, OpenAgent:

1. validates the call against registered tool definitions;
2. checks permission rules;
3. executes the tool through the current runtime;
4. records a `tool-result` event;
5. feeds the result back into the next model step.

Workspace tools such as `bash`, `read`, `write`, `edit`, `grep`, `glob`, and `ls` use `WorkspaceRuntime`. Non-workspace tools such as MCP, web, skill, todo, and question are runtime-agnostic unless the tool declares otherwise.

## Provider Boundary

Providers implement the `LanguageModel` protocol:

```python
async def stream(*, system, messages, tools, temperature=None, max_output_tokens=None, options=None):
    ...
```

The loop does not depend on any provider SDK directly. Provider-specific wire formats are translated into OpenAgent stream events before they reach the loop.

## Public Scope

This repository is the core runtime. CLI and Web Console experiments are intentionally outside the public core and should live in a separate package if restored.
