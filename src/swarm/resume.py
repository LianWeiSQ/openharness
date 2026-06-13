from __future__ import annotations

"""Resume policy for swarm coordinator runs."""

from dataclasses import dataclass, replace
from typing import Any


DEFAULT_REUSE_STATUSES = ("completed",)
VALID_STATUSES = {"completed", "partial", "failed", "cancelled"}


@dataclass(frozen=True, slots=True)
class SwarmResumePolicy:
    enabled: bool = False
    reuse_statuses: tuple[str, ...] = DEFAULT_REUSE_STATUSES
    strict_task_id: bool = True
    strict_load: bool = False

    def should_reuse(self, status: str) -> bool:
        return self.enabled and status in set(self.reuse_statuses)


def resolve_resume_policy(default: SwarmResumePolicy | bool | dict[str, Any] | None, task_metadata: dict[str, Any]) -> SwarmResumePolicy:
    policy = resume_policy_from_value(default)
    if "resume" in task_metadata:
        policy = resume_policy_from_value(task_metadata.get("resume"), base=policy)
    return policy


def resume_policy_from_value(value: SwarmResumePolicy | bool | dict[str, Any] | None, *, base: SwarmResumePolicy | None = None) -> SwarmResumePolicy:
    policy = base or SwarmResumePolicy()
    if value is None:
        return policy
    if isinstance(value, SwarmResumePolicy):
        return value
    if isinstance(value, bool):
        return replace(policy, enabled=value)
    if not isinstance(value, dict):
        raise ValueError("swarm resume policy must be a boolean or mapping")

    reuse_statuses = value.get("reuse_statuses", policy.reuse_statuses)
    if isinstance(reuse_statuses, str):
        parsed_statuses = (reuse_statuses,)
    elif isinstance(reuse_statuses, (list, tuple, set)):
        parsed_statuses = tuple(str(item) for item in reuse_statuses)
    else:
        raise ValueError("swarm resume reuse_statuses must be a string or list")
    invalid = sorted(set(parsed_statuses) - VALID_STATUSES)
    if invalid:
        raise ValueError(f"swarm resume reuse_statuses contains invalid status: {', '.join(invalid)}")

    return replace(
        policy,
        enabled=bool(value.get("enabled", policy.enabled)),
        reuse_statuses=parsed_statuses,
        strict_task_id=bool(value.get("strict_task_id", policy.strict_task_id)),
        strict_load=bool(value.get("strict_load", policy.strict_load)),
    )


__all__ = ["SwarmResumePolicy", "resolve_resume_policy", "resume_policy_from_value"]
