from __future__ import annotations

"""Combined coordinator workflow for swarm runs."""

from dataclasses import dataclass, field
from typing import Any
from uuid import uuid4

from .config import TaskConfig
from .merge import MergeApplyResult, MergePlan, apply_merge_plan, build_merge_plan
from .merge_policy import MergeApprovalDecision, MergeApprovalPolicy, evaluate_merge_plan, merge_approval_policy_from_metadata
from .runtime import SwarmRunResult, SwarmRuntime
from .team import FileTeamHandoffStore, TeamHandoff, build_team_handoff, task_for_team_handoff_resume


@dataclass(frozen=True, slots=True)
class SwarmCoordinatorOptions:
    run_id: str | None = None
    resume_from_handoff: bool = True
    pending_only_resume: bool = False
    save_team_handoff: bool = True
    merge_enabled: bool = False
    merge_source_root: str | None = None
    merge_target_root: str | None = None
    merge_policy: MergeApprovalPolicy | dict[str, Any] | None = None
    apply_approved_merge: bool = False


@dataclass(frozen=True, slots=True)
class SwarmCoordinatorReceipt:
    run_id: str
    task_id: str
    run_status: str
    handoff_saved: bool = False
    handoff_path: str | None = None
    handoff_has_pending: bool = False
    pending_runner_ids: list[str] = field(default_factory=list)
    reusable_runner_ids: list[str] = field(default_factory=list)
    merge_enabled: bool = False
    merge_decision: str | None = None
    merge_reason_codes: list[str] = field(default_factory=list)
    merge_change_count: int = 0
    merge_conflict_count: int = 0
    merge_applied_count: int = 0
    warnings: list[str] = field(default_factory=list)
    diagnostics: list[str] = field(default_factory=list)

    def as_dict(self) -> dict[str, Any]:
        return {
            "run_id": self.run_id,
            "task_id": self.task_id,
            "run_status": self.run_status,
            "handoff_saved": self.handoff_saved,
            "handoff_path": self.handoff_path,
            "handoff_has_pending": self.handoff_has_pending,
            "pending_runner_ids": list(self.pending_runner_ids),
            "reusable_runner_ids": list(self.reusable_runner_ids),
            "merge_enabled": self.merge_enabled,
            "merge_decision": self.merge_decision,
            "merge_reason_codes": list(self.merge_reason_codes),
            "merge_change_count": self.merge_change_count,
            "merge_conflict_count": self.merge_conflict_count,
            "merge_applied_count": self.merge_applied_count,
            "warnings": list(self.warnings),
            "diagnostics": list(self.diagnostics),
        }


@dataclass(frozen=True, slots=True)
class SwarmCoordinatorResult:
    run_result: SwarmRunResult
    handoff: TeamHandoff
    receipt: SwarmCoordinatorReceipt
    merge_plan: MergePlan | None = None
    merge_decision: MergeApprovalDecision | None = None
    merge_apply_result: MergeApplyResult | None = None


async def run_swarm_coordinator(
    *,
    runtime: SwarmRuntime,
    task: TaskConfig,
    options: SwarmCoordinatorOptions | None = None,
    team_handoff_store: FileTeamHandoffStore | None = None,
) -> SwarmCoordinatorResult:
    resolved_options = options or SwarmCoordinatorOptions()
    run_id = resolved_options.run_id or f"swarm_{uuid4().hex}"
    diagnostics: list[str] = []
    task_to_run = task

    if resolved_options.resume_from_handoff and team_handoff_store is not None:
        previous = _load_existing_handoff(team_handoff_store, run_id, diagnostics)
        if previous is not None:
            task_to_run = task_for_team_handoff_resume(
                task=task,
                handoff=previous,
                pending_only=resolved_options.pending_only_resume,
            )

    run_result = await runtime.run_task(task_to_run, run_id=run_id)
    handoff = build_team_handoff(task=task, result=run_result, run_id=run_id)

    handoff_saved = False
    handoff_path = None
    if resolved_options.save_team_handoff and team_handoff_store is not None:
        team_handoff_store.save_handoff(handoff)
        handoff_saved = True
        handoff_path = str(team_handoff_store.handoff_path(run_id))

    merge_plan: MergePlan | None = None
    merge_decision: MergeApprovalDecision | None = None
    merge_apply_result: MergeApplyResult | None = None
    if resolved_options.merge_enabled:
        try:
            merge_plan = build_merge_plan(run_result.results, source_root=resolved_options.merge_source_root)
            policy = resolved_options.merge_policy or merge_approval_policy_from_metadata(task.metadata)
            merge_decision = evaluate_merge_plan(merge_plan, policy)
            if resolved_options.apply_approved_merge and merge_decision.can_apply:
                merge_apply_result = apply_merge_plan(merge_plan, target_root=resolved_options.merge_target_root)
        except Exception as error:  # noqa: BLE001
            diagnostics.append(f"merge workflow failed: {error}")

    receipt = SwarmCoordinatorReceipt(
        run_id=run_id,
        task_id=task.id,
        run_status=run_result.status,
        handoff_saved=handoff_saved,
        handoff_path=handoff_path,
        handoff_has_pending=handoff.has_pending,
        pending_runner_ids=list(handoff.pending_runner_ids),
        reusable_runner_ids=list(handoff.reusable_runner_ids),
        merge_enabled=resolved_options.merge_enabled,
        merge_decision=merge_decision.status if merge_decision else None,
        merge_reason_codes=[reason.code for reason in merge_decision.reasons] if merge_decision else [],
        merge_change_count=len(merge_plan.changes) if merge_plan else 0,
        merge_conflict_count=len(merge_plan.conflicts) if merge_plan else 0,
        merge_applied_count=len(merge_apply_result.applied) if merge_apply_result else 0,
        warnings=list(run_result.warnings),
        diagnostics=diagnostics,
    )
    return SwarmCoordinatorResult(
        run_result=run_result,
        handoff=handoff,
        receipt=receipt,
        merge_plan=merge_plan,
        merge_decision=merge_decision,
        merge_apply_result=merge_apply_result,
    )


def _load_existing_handoff(store: FileTeamHandoffStore, run_id: str, diagnostics: list[str]) -> TeamHandoff | None:
    try:
        return store.load_handoff(run_id)
    except FileNotFoundError:
        return None
    except Exception as error:  # noqa: BLE001
        diagnostics.append(f"team handoff load failed: {error}")
        return None


__all__ = [
    "SwarmCoordinatorOptions",
    "SwarmCoordinatorReceipt",
    "SwarmCoordinatorResult",
    "run_swarm_coordinator",
]
