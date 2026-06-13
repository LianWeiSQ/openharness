from __future__ import annotations

"""Minimal supervisor runtime for the swarm kernel."""

import asyncio
from dataclasses import dataclass, field
from uuid import uuid4

from .config import TaskConfig
from .protocol import AgentResult, AgentSpec, FanoutBudget, RunContext, Usage
from .registry import RunnerRegistry


@dataclass(frozen=True, slots=True)
class SwarmRunResult:
    task_id: str
    status: str
    summary: str
    results: dict[str, AgentResult]
    usage: Usage = field(default_factory=Usage)
    warnings: list[str] = field(default_factory=list)


class SwarmRuntime:
    def __init__(self, *, registry: RunnerRegistry, fanout_budget: FanoutBudget | None = None) -> None:
        self.registry = registry
        self.fanout_budget = (fanout_budget or FanoutBudget()).normalized()

    async def run_task(self, task: TaskConfig, *, run_id: str | None = None) -> SwarmRunResult:
        runners = self._resolve_runners(task)
        warnings: list[str] = []
        if len(runners) > self.fanout_budget.max_total_workers:
            warnings.append(
                f"runner count {len(runners)} exceeds max_total_workers {self.fanout_budget.max_total_workers}; truncating"
            )
            runners = runners[: self.fanout_budget.max_total_workers]

        semaphore = asyncio.Semaphore(self.fanout_budget.max_concurrent)
        context = RunContext(run_id=run_id or f"swarm_{uuid4().hex}", budget=self.fanout_budget)

        async def run_one(runner_id: str) -> tuple[str, AgentResult]:
            runner = self.registry.require(runner_id)
            role = task.role if runner.descriptor.supports_role(task.role) else (runner.descriptor.roles[0] if runner.descriptor.roles else task.role)
            spec = AgentSpec(
                role=role,
                objective=task.objective,
                context=task.context,
                boundaries=task.boundaries,
                output_schema=task.output_schema,
                inputs=dict(task.inputs),
                limits=task.limits,
                permissions=task.permissions,
                metadata={**task.metadata, "task_id": task.id, "runner_id": runner_id},
            )
            try:
                async with semaphore:
                    handle = await runner.start(spec, context)
                    result = await handle.result()
                    return runner_id, result
            except Exception as error:  # noqa: BLE001
                return runner_id, AgentResult(
                    status="failed",
                    summary=str(error),
                    metadata={"error_kind": "runner_dispatch_error", "runner_id": runner_id},
                )

        pairs = await asyncio.gather(*(run_one(runner_id) for runner_id in runners))
        results = {runner_id: result for runner_id, result in pairs}
        usage = Usage()
        for result in results.values():
            usage = usage.plus(result.usage)

        warnings.extend(_budget_warnings(usage, self.fanout_budget))
        return SwarmRunResult(
            task_id=task.id,
            status=_aggregate_status(results),
            summary=_aggregate_summary(results),
            results=results,
            usage=usage,
            warnings=warnings,
        )

    def _resolve_runners(self, task: TaskConfig) -> list[str]:
        if task.runner_ids:
            for runner_id in task.runner_ids:
                self.registry.require(runner_id)
            return list(task.runner_ids)

        matches = self.registry.matching_role(task.role)
        if not matches:
            raise KeyError(f'no runner matches role "{task.role}"')
        return [runner.descriptor.id for runner in matches]


def _aggregate_status(results: dict[str, AgentResult]) -> str:
    if not results:
        return "failed"
    statuses = {result.status for result in results.values()}
    if statuses == {"completed"}:
        return "completed"
    if "completed" in statuses or "partial" in statuses:
        return "partial"
    if "cancelled" in statuses:
        return "cancelled"
    return "failed"


def _aggregate_summary(results: dict[str, AgentResult]) -> str:
    if not results:
        return "No runners executed."
    lines = []
    for runner_id, result in results.items():
        lines.append(f"[{runner_id}] {result.status}: {result.summary}")
    return "\n".join(lines)


def _budget_warnings(usage: Usage, budget: FanoutBudget) -> list[str]:
    warnings: list[str] = []
    if budget.max_total_tokens is not None and usage.total_tokens > budget.max_total_tokens:
        warnings.append(f"total tokens {usage.total_tokens} exceeded budget {budget.max_total_tokens}")
    if budget.max_total_cost is not None and usage.cost > budget.max_total_cost:
        warnings.append(f"total cost {usage.cost} exceeded budget {budget.max_total_cost}")
    return warnings
