from __future__ import annotations

"""Shared wire payload helpers for external swarm runners."""

from typing import Any

from .protocol import AgentDescriptor, AgentSpec, RunContext


def payload_for_runner(
    *,
    spec: AgentSpec,
    ctx: RunContext,
    descriptor: AgentDescriptor,
    runner_metadata: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "spec": {
            "role": spec.role,
            "objective": spec.objective,
            "context": spec.context,
            "boundaries": spec.boundaries,
            "output_schema": spec.output_schema,
            "inputs": spec.inputs,
            "permissions": spec.permissions,
            "metadata": spec.metadata,
        },
        "context": {
            "run_id": ctx.run_id,
            "parent_span_id": ctx.parent_span_id,
            "metadata": ctx.metadata,
        },
        "runner": {
            "id": descriptor.id,
            "kind": descriptor.kind,
            "roles": descriptor.roles,
            "metadata": dict(descriptor.metadata if runner_metadata is None else runner_metadata),
        },
    }


__all__ = ["payload_for_runner"]
