from __future__ import annotations

"""Coordinator approval policy for swarm merge plans."""

import fnmatch
from dataclasses import dataclass, field, replace
from typing import Any, Literal

from .merge import MergeChange, MergePlan

MergeApprovalStatus = Literal["approved", "needs_review", "rejected"]


@dataclass(frozen=True, slots=True)
class MergeApprovalReason:
    code: str
    message: str
    relative_path: str | None = None
    runner_ids: tuple[str, ...] = ()
    actual: int | float | None = None
    limit: int | float | None = None


@dataclass(frozen=True, slots=True)
class MergeApprovalPolicy:
    auto_approve: bool = False
    allow_conflicts: bool = False
    allow_deletions: bool = False
    max_changed_files: int | None = None
    max_total_bytes: int | None = None
    protected_paths: tuple[str, ...] = ()
    reject_on_policy_violation: bool = True


@dataclass(frozen=True, slots=True)
class MergeApprovalDecision:
    status: MergeApprovalStatus
    reasons: list[MergeApprovalReason] = field(default_factory=list)
    approved_changes: list[MergeChange] = field(default_factory=list)
    blocked_changes: list[MergeChange] = field(default_factory=list)

    @property
    def can_apply(self) -> bool:
        return self.status == "approved"


def evaluate_merge_plan(plan: MergePlan, policy: MergeApprovalPolicy | dict[str, Any] | None = None) -> MergeApprovalDecision:
    resolved_policy = merge_approval_policy_from_value(policy)
    reasons: list[MergeApprovalReason] = []
    blocked_paths: set[str] = set()

    if plan.conflicts and not resolved_policy.allow_conflicts:
        for conflict in plan.conflicts:
            blocked_paths.add(conflict.relative_path)
            reasons.append(
                MergeApprovalReason(
                    code="conflict",
                    message=f'path "{conflict.relative_path}" has conflicting worker changes',
                    relative_path=conflict.relative_path,
                    runner_ids=tuple(conflict.runner_ids),
                )
            )

    deletion_changes = [change for change in plan.changes if change.change_type == "deleted"]
    if deletion_changes and not resolved_policy.allow_deletions:
        for change in deletion_changes:
            blocked_paths.add(change.relative_path)
            reasons.append(
                MergeApprovalReason(
                    code="deletion",
                    message=f'path "{change.relative_path}" deletes a source file',
                    relative_path=change.relative_path,
                    runner_ids=(change.runner_id,),
                )
            )

    for change in plan.changes:
        matched = _matching_protected_pattern(change.relative_path, resolved_policy.protected_paths)
        if matched:
            blocked_paths.add(change.relative_path)
            reasons.append(
                MergeApprovalReason(
                    code="protected_path",
                    message=f'path "{change.relative_path}" matches protected pattern "{matched}"',
                    relative_path=change.relative_path,
                    runner_ids=(change.runner_id,),
                )
            )

    changed_file_count = len({change.relative_path for change in plan.changes})
    if resolved_policy.max_changed_files is not None and changed_file_count > resolved_policy.max_changed_files:
        reasons.append(
            MergeApprovalReason(
                code="max_changed_files",
                message=f"changed file count {changed_file_count} exceeds limit {resolved_policy.max_changed_files}",
                actual=changed_file_count,
                limit=resolved_policy.max_changed_files,
            )
        )
        blocked_paths.update(change.relative_path for change in plan.changes)

    total_bytes = _unique_changed_bytes(plan.changes)
    if resolved_policy.max_total_bytes is not None and total_bytes > resolved_policy.max_total_bytes:
        reasons.append(
            MergeApprovalReason(
                code="max_total_bytes",
                message=f"changed bytes {total_bytes} exceeds limit {resolved_policy.max_total_bytes}",
                actual=total_bytes,
                limit=resolved_policy.max_total_bytes,
            )
        )
        blocked_paths.update(change.relative_path for change in plan.changes)

    blocked_changes = [change for change in plan.changes if change.relative_path in blocked_paths]
    if blocked_changes:
        status: MergeApprovalStatus = "rejected" if resolved_policy.reject_on_policy_violation else "needs_review"
        return MergeApprovalDecision(status=status, reasons=reasons, blocked_changes=blocked_changes)

    if not plan.changes:
        return MergeApprovalDecision(
            status="approved",
            reasons=[MergeApprovalReason(code="no_changes", message="merge plan has no changes")],
            approved_changes=[],
        )

    if resolved_policy.auto_approve:
        return MergeApprovalDecision(
            status="approved",
            reasons=[MergeApprovalReason(code="auto_approved", message="merge plan matched auto-approval policy")],
            approved_changes=list(plan.changes),
        )

    return MergeApprovalDecision(
        status="needs_review",
        reasons=[MergeApprovalReason(code="manual_review_required", message="merge plan requires coordinator review")],
    )


def merge_approval_policy_from_metadata(metadata: dict[str, Any]) -> MergeApprovalPolicy:
    merge = metadata.get("merge") if isinstance(metadata.get("merge"), dict) else {}
    approval = merge.get("approval") if isinstance(merge, dict) and isinstance(merge.get("approval"), dict) else None
    return merge_approval_policy_from_value(approval or metadata.get("merge_approval"))


def merge_approval_policy_from_value(value: MergeApprovalPolicy | dict[str, Any] | None) -> MergeApprovalPolicy:
    if value is None:
        return MergeApprovalPolicy()
    if isinstance(value, MergeApprovalPolicy):
        return value
    if not isinstance(value, dict):
        raise ValueError("merge approval policy must be a mapping")

    policy = MergeApprovalPolicy()
    return replace(
        policy,
        auto_approve=bool(value.get("auto_approve", policy.auto_approve)),
        allow_conflicts=bool(value.get("allow_conflicts", policy.allow_conflicts)),
        allow_deletions=bool(value.get("allow_deletions", policy.allow_deletions)),
        max_changed_files=_optional_int(value.get("max_changed_files")),
        max_total_bytes=_optional_int(value.get("max_total_bytes")),
        protected_paths=_string_tuple(value.get("protected_paths") or ()),
        reject_on_policy_violation=bool(value.get("reject_on_policy_violation", policy.reject_on_policy_violation)),
    )


def _matching_protected_pattern(relative_path: str, patterns: tuple[str, ...]) -> str:
    normalized = relative_path.strip("/")
    for pattern in patterns:
        candidate = pattern.strip("/")
        if not candidate:
            continue
        if normalized == candidate or fnmatch.fnmatch(normalized, candidate):
            return pattern
        if candidate.endswith("/**") and normalized.startswith(candidate[:-3].rstrip("/") + "/"):
            return pattern
    return ""


def _unique_changed_bytes(changes: list[MergeChange]) -> int:
    by_path: dict[str, int] = {}
    for change in changes:
        by_path[change.relative_path] = max(by_path.get(change.relative_path, 0), int(change.size_bytes or 0))
    return sum(by_path.values())


def _optional_int(value: Any) -> int | None:
    if value is None:
        return None
    return int(value)


def _string_tuple(value: Any) -> tuple[str, ...]:
    if isinstance(value, str):
        return (value,)
    if isinstance(value, (list, tuple, set)):
        return tuple(str(item) for item in value)
    raise ValueError("merge approval protected_paths must be a string or list")


__all__ = [
    "MergeApprovalDecision",
    "MergeApprovalPolicy",
    "MergeApprovalReason",
    "MergeApprovalStatus",
    "evaluate_merge_plan",
    "merge_approval_policy_from_metadata",
    "merge_approval_policy_from_value",
]
