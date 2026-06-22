# Swarm Kernel: an agent-agnostic multi-agent runtime

Status: Draft / proposal
Last updated: 2026-06-12
Working package name: `swarm` (standalone; see §8)
Task queue: `tasks.json` (`SW-001..SW-008` kernel, `OA-018..OA-021` openagent adapter)
Human-readable follow-ups: `doc/maintenance.md`

This document proposes a **standalone, agent-agnostic swarm/team orchestration
kernel**. The kernel does not know what an "agent" is internally — it talks to
agents through a thin **runner protocol**. OpenAgent is the *reference adapter*,
not the host. Any other agent (a CLI agent, an HTTP/A2A endpoint, an OpenAI
Agents SDK service, a LangGraph server) can join the same swarm by implementing
the same protocol.

> Analogy: MCP standardized how an agent reaches **tools**. This kernel
> standardizes how a coordinator reaches **agents**. The pluggable boundary is
> the whole point — OpenAgent is one citizen of the swarm, not its container.

---

## 1. Vision and principles

- **Decoupled.** The kernel is a separate package with **zero dependency on
  openagent**. OpenAgent depends on the kernel (optional extra), never the
  reverse. The kernel is usable with only remote/HTTP agents and no openagent
  installed; openagent is usable with no kernel installed (single-agent).
- **Protocol-first.** A small, stable `AgentRunner` protocol is the contract.
  Everything else (topologies, scheduling, trace) is built on top of it.
- **Transport-agnostic.** A runner may execute in-process, in a subprocess, or
  over the network. The kernel does not care.
- **Start centralized, read-only, observable.** Supervisor topology first,
  read-only fan-out first, trace lineage first. Decentralized swarm and
  write-capable agents are later, opt-in phases.
- **Data boundary preserved.** Kernel trace defaults to metadata-only; prompt /
  output / tool-IO content require explicit opt-in (same rule as OA-016).

### Non-goals (early phases)

- No write-capable workers until a file-concurrency policy exists (§7, Phase 3).
- No nested swarms in v0 (a worker cannot itself spawn a swarm).
- No persistent cross-session teams until a Store backend exists (Phase 4).

---

## 2. Architecture: two layers

```
┌────────────────────────────────────────────────────────────────┐
│  SWARM KERNEL  (package: swarm/ — no openagent import)          │
│                                                                 │
│   Registry ──> Scheduler ──> Topology Strategy ──> Aggregator   │
│   (descriptors) (concurrency,  (supervisor /        (synthesis) │
│                  budget)        hierarchical /                   │
│                                 decentralized)                   │
│                       │                                          │
│                 Trace (agent-agnostic span lineage)             │
└───────────────────────┬─────────────────────────────────────────┘
                        │  AgentRunner protocol (start / events / result / cancel)
        ┌───────────────┼───────────────────────────┐
        ▼               ▼                           ▼
  OpenAgentRunner   SubprocessRunner            HttpRunner / A2ARunner
  (in-process       (any CLI agent via          (remote agent endpoints,
   AgentLoop)        stdio JSON)                  A2A "Agent Cards")
        │
        ▼
   openagent core (AgentLoop, tools, trace) — one citizen of the swarm
```

**Dependency direction (strict):** `openagent → swarm`. The kernel never imports
openagent; openagent provides `OpenAgentRunner` implementing the kernel's
protocol.

---

## 3. The `AgentRunner` protocol (the contract)

The kernel's single source of truth. Named `AgentRunner` to avoid collision with
openagent's existing `AgentAdapter` (model+config reply-stream adapter).

```python
# swarm/protocol.py  (illustrative)

@dataclass
class AgentSpec:                 # the contract handed to a worker
    role: str                    # "explore" | "research" | "review" | ...
    objective: str               # the single outcome the worker owns
    context: str = ""            # minimal facts (worker can't see lead history)
    boundaries: str = ""         # out-of-scope + allowed tools/sources
    inputs: dict = field(default_factory=dict)   # artifact refs, files, urls
    output_schema: dict | None = None            # required result shape
    limits: RunLimits = field(default_factory=RunLimits)  # steps/tokens/cost/timeout
    permissions: str = "READONLY"

@dataclass
class AgentResult:               # the compressed, machine-checkable return
    status: Literal["completed", "partial", "failed", "cancelled"]
    summary: str
    evidence: list[str] = field(default_factory=list)      # file:line / url anchors
    open_questions: list[str] = field(default_factory=list)
    confidence: float = 0.0
    artifacts: list[ArtifactRef] = field(default_factory=list)
    usage: Usage = field(default_factory=Usage)            # tokens/cost/steps/latency
    handoff: HandoffRequest | None = None                  # for swarm topology

@dataclass
class AgentDescriptor:           # capability advertisement (like an A2A Agent Card)
    id: str
    roles: list[str]
    tool_groups: list[str]
    model_tier: str              # e.g. "lead" | "worker"
    max_context: int
    supports_streaming: bool

class AgentRunHandle(Protocol):
    def events(self) -> AsyncIterator[AgentEvent]: ...   # streamed progress/trace
    async def result(self) -> AgentResult: ...
    async def cancel(self) -> None: ...

class AgentRunner(Protocol):
    @property
    def descriptor(self) -> AgentDescriptor: ...
    async def start(self, spec: AgentSpec, ctx: RunContext) -> AgentRunHandle: ...
```

`RunContext` is agent-agnostic and carries trace identity and control:
`run_id`, `parent_span_id`, a cancellation signal, and a budget token. Adapters
map their native events into kernel `AgentEvent`s; a rich runner (openagent)
forwards detailed trace, a coarse runner (HTTP) maps start/finish/error.

### The four-part contract (anti-drift)

Every `AgentSpec` must carry `objective`, `output_schema`, `context`, and
`boundaries`. Missing any one is the documented cause of worker drift. The
kernel validates this before dispatch, regardless of which runner executes it.

---

## 4. The kernel

- **Registry** — runners registered by id and advertised `AgentDescriptor`.
  Routing/scheduling decisions use descriptors (role match, cost tier, context
  size, streaming support).
- **Scheduler** — bounded concurrency + a `FanoutBudget` (max concurrent, max
  total workers per run, aggregate token/cost ceiling). Budget breaches surface
  as warnings, mirroring openagent's `runtime_warnings` slice.
- **Topology strategy (pluggable)** — a policy over *what to dispatch, how to
  route results/handoffs, when to stop*:
  - `Supervisor` (v0): lead decomposes → dispatch workers → aggregate.
  - `Hierarchical`: role chains (Researcher → Builder → Reviewer).
  - `Decentralized` (optional, last): peer handoff with ping-pong detection.
- **Trace** — agent-agnostic events with `span_id` / `parent_span_id` lineage.
  The kernel owns the swarm-level span tree; each runner nests under its spawn
  span. Metadata-only by default.
- **Aggregator** — collects `AgentResult`s into a synthesis. The aggregator may
  itself be an `AgentRunner` call (e.g. a "synthesis" role).

---

## 5. Runner kinds (how "any agent" plugs in)

| Runner | Transport | Use | Trace fidelity |
| --- | --- | --- | --- |
| `OpenAgentRunner` | in-process | reference; lowest latency, richest trace | full |
| `SubprocessRunner` | stdio JSON | any CLI agent (`openagent run --json`, others) | medium |
| `HttpRunner` / `A2ARunner` | HTTP / A2A | remote agents, mixed-vendor swarms | coarse |

"OpenAgent is one of them" is realized concretely: `OpenAgentRunner` is the
in-process reference; everything else joins via subprocess or HTTP/A2A
implementing the identical protocol. A swarm can be **heterogeneous** — an
openagent worker, an external research agent, and a code agent in one team.

---

## 6. How OpenAgent plugs in (the reference adapter)

OpenAgent already exposes the hooks a runner needs; the adapter is additive.

| Kernel need | Existing openagent hook |
| --- | --- |
| Subagent role flag | `AgentConfig.mode = "primary" \| "subagent"` (`core/types.py:100`) |
| Read-only tool scope | `AgentLoop._tools_for_agent` maps `tools="readonly"` → `{read, glob, grep, ls, skill, todoread, question}` (`core/loop/processor.py:205`) |
| Isolated context | each agent owns a `Session` + `ContextPack` |
| Per-session workspace | `build_workspace_runtime(session)` (`core/execution/runtime.py:80`) |
| Trace lineage | `TraceEvent.span_id` / `parent_span_id` already layered |

`OpenAgentRunner.start(spec, ctx)`:
1. builds a `Session` (forked dir / shared read-only workspace),
2. builds an `AgentLoop` with `mode="subagent"`, `tools="readonly"`, the lead's
   model, and a `READONLY` permission ruleset,
3. maps the loop's stream events into kernel `AgentEvent`s, stamping
   `parent_span_id = ctx.parent_span_id`,
4. returns an `AgentRunHandle`; on completion produces an `AgentResult`.

**The lead's `spawn_agent` tool no longer hardcodes spawning.** It becomes a thin
client that calls the **kernel** (a kernel client handle injected into
`ToolContext.extra["swarm"]` — the only new wire, backward-compatible, no global
singletons). The lead asks the kernel to run specs; the kernel picks runners.

---

## 7. Complete evolution path

Each phase ships independently. Phases 0–1 are single-process and read-only.

### Phase 0 — Protocol + in-process supervisor (kernel MVP)
- `SW-001` Define `AgentRunner` / `AgentSpec` / `AgentResult` / `AgentDescriptor`
  / `RunContext` in standalone `swarm/` (zero openagent deps).
- `SW-002` `Supervisor` topology + scheduler + `FanoutBudget` + concurrency cap.
- `OA-018` `OpenAgentRunner` (in-process) over `AgentLoop`, `tools="readonly"`.
- `OA-019` `spawn_agent` tool → kernel client via `ToolContext.extra["swarm"]`.
- Hard-blocks on `OA-002` (single ContextPackBuilder assembly path).
- **Outcome:** read-only parallel fan-out, no writes, no persistence, no new
  trace schema. Equivalent capability to the earlier single-host v1, but the
  spawning path is now the agent-agnostic kernel.

### Phase 1 — Kernel hardening (observability + routing + eval)
- `SW-003` Agent-agnostic trace lineage + capability descriptors/routing.
- `OA-020` Map openagent trace events into kernel lineage.
- `SW-007` Topology-agnostic LLM-as-judge + rubric eval harness (no single
  correct path in multi-agent; needed before any writes).

### Phase 2 — Out-of-process runners (the "any agent" unlock)
- `SW-004` `SubprocessRunner` (CLI agents via stdio JSON).
- `SW-005` `HttpRunner` / `A2ARunner` (remote agents, A2A Agent Cards).
- **Outcome:** non-openagent agents join the swarm; mixed-vendor teams possible.

### Phase 3 — Topologies + heterogeneous, write-capable teams
- `SW-006` `Hierarchical` topology + structured handoff schema
  (`summary / evidence / open_questions / confidence / tool_state`) +
  ping-pong loop detection + reviewer/critic role.
- File-concurrency policy for write-capable runners: git-worktree isolation or
  directory ownership (worktree reuses openagent's per-session workspace).

### Phase 4 — Persistent teams + decentralized swarm (optional)
- `SW-008` Persistent team `Store` protocol (kernel side) + optional
  `Decentralized` peer-handoff topology.
- `OA-021` openagent implements the kernel `Store` via persistent session
  storage (`OA-003`), enabling cross-session pause/resume of team state and a
  shared blackboard memory pool.

### Recommended global order

```
OA-002 ─┐
        ├─ SW-001 ─ SW-002 ─ OA-018 ─ OA-019   (Phase 0)
        ├─ SW-003 ─ OA-020 ─ SW-007            (Phase 1)
        ├─ SW-004 ─ SW-005                     (Phase 2)
        ├─ SW-006                              (Phase 3)
        └─ OA-003 ─ SW-008 ─ OA-021            (Phase 4)
```

---

## 8. Packaging and boundaries

**Recommended (to confirm):** a standalone in-repo package `src/swarm/` with an
**enforced zero-import rule** (no `import openagent` anywhere under `swarm/`),
its own tests, and — when it stabilizes — its own `pyproject`/repo. This buys the
decoupling discipline immediately without repo-split overhead, and the clean
import boundary makes a later extraction mechanical.

Alternatives considered:
- *Separate repo from day 1* — cleanest boundary, highest coordination overhead
  now (two repos, version pinning before the protocol is stable).
- *Build inside openagent core, extract later* — fastest, but coupling creep
  directly contradicts the "swarm is standalone" goal.

A CI check should assert `swarm/` has no openagent imports, locking the boundary.

---

## 9. Design decisions (resolved 2026-06-12)

1. **Dependency direction** — `openagent → swarm`, never the reverse. Kernel is
   standalone.
2. **Protocol name** — `AgentRunner` (avoids collision with the existing
   `AgentAdapter`).
3. **Kernel injection into openagent** — `ToolContext.extra["swarm"]`
   (backward-compatible, no global singleton).
4. **Worker model (v0)** — workers use the **same model as the lead**; no
   per-role model override yet. Cost controlled purely by `FanoutBudget`.
   Per-role model selection deferred to the role library (Phase 3).
5. **Result format** — lead receives `AgentResult` as **compact JSON** so it can
   read `confidence` / `open_questions` programmatically and minimize tokens.
6. **Failure semantics** — runner error / timeout / budget-exhaustion returns a
   `status`-tagged partial `AgentResult`; it **never raises into the lead loop**.
7. **Start topology** — `Supervisor` only in Phase 0; decentralized swarm is the
   last, optional phase.
8. **Eval** — topology-agnostic LLM-as-judge + rubric (Phase 1), required before
   write-capable runners.

### Open question

- **Out-of-process transport ordering** — `SubprocessRunner` (stdio JSON) first
  for fast local heterogeneous tests, vs `A2ARunner` first to align with the
  emerging A2A standard. Current lean: subprocess first (cheaper to prove),
  A2A second.

---

## 10. Verification

Kernel (standalone, no model needed for protocol/topology unit tests):

```bash
PYTHONPATH=src:src/tests python -m unittest discover -s src/tests -p "test_swarm_*.py"
```

OpenAgent adapter integration must keep the full suite green:

```bash
PYTHONPATH=src:src/tests python -m unittest src/tests/test_loop.py
PYTHONPATH=src:src/tests python -m unittest discover -s src/tests -p "test_*.py"
```

Boundary guard (no openagent import inside the kernel):

```bash
rg -n "import openagent|from openagent" src/swarm || echo "kernel boundary clean"
```

Targeted tests to add:
- `swarm`: protocol validation rejects an `AgentSpec` missing any contract field;
  `Supervisor` dispatches/aggregates; `FanoutBudget` blocks past caps and warns;
  a failing runner yields a `status="failed"` partial, never raises.
- `openagent`: `OpenAgentRunner` with `tools="readonly"` cannot call write/bash;
  spawned worker trace events carry `parent_span_id` linking to the lead span.

---

## 11. References

- Anthropic, "How we built our multi-agent research system" — orchestrator-worker,
  parallel subagents (~15x token cost; weaker for tightly-coupled coding).
  https://www.anthropic.com/engineering/multi-agent-research-system
- Claude Code subagents — isolated context, single-summary return, no nesting,
  worktree isolation.
- OpenAI Swarm (educational, superseded by the Agents SDK) — lightweight
  agent-to-agent handoff. https://github.com/openai/swarm
- A2A (Agent-to-Agent) protocol — agent capability cards and cross-vendor
  orchestration; candidate transport for `A2ARunner`.
- Agent team design patterns (supervisor / swarm / hierarchical), handoff schema
  and loop detection.
  https://www.padiso.ai/blog/agent-team-design-patterns-supervisor-swarm-hierarchical
- awesome-agent-swarm landscape. https://github.com/EvoMap/awesome-agent-swarm
</content>
