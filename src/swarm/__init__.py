from __future__ import annotations

"""Agent-agnostic swarm/function kernel.

The swarm package intentionally has no dependency on openagent. OpenAgent can
adapt into this kernel later, but the kernel should remain usable with plain
Python functions, subprocess agents, HTTP agents, or any other runner.
"""

from .config import RunnerConfig, SwarmConfig, TaskConfig, load_swarm_config
from .a2a_runner import A2ARequestConfig, A2ARunner, build_a2a_registry
from .coordinator import SwarmCoordinatorOptions, SwarmCoordinatorReceipt, SwarmCoordinatorResult, run_swarm_coordinator
from .function_runner import FunctionRunner, build_function_registry
from .http_runner import HttpRequestConfig, HttpRunner, build_http_registry
from .inspection import (
    COORDINATOR_RECEIPT_FILE,
    SwarmInspectionConfig,
    create_inspection_server,
    load_run_artifact,
    load_run_detail,
    load_run_index,
    serve_inspection_api,
    write_coordinator_receipt,
)
from .isolation import WorkerWorkspace, WorkerWorkspaceConfig, prepare_worker_workspace, resolve_worker_workspace_config
from .langfuse_exporter import SwarmLangfuseExportResult, SwarmLangfuseExporter, export_swarm_trace_to_langfuse
from .merge import MergeApplyResult, MergeChange, MergeConflict, MergePlan, apply_merge_plan, build_merge_plan
from .merge_policy import (
    MergeApprovalDecision,
    MergeApprovalPolicy,
    MergeApprovalReason,
    MergeApprovalStatus,
    evaluate_merge_plan,
    merge_approval_policy_from_metadata,
    merge_approval_policy_from_value,
)
from .protocol import (
    AgentDescriptor,
    AgentEvent,
    AgentResult,
    AgentRunHandle,
    AgentRunner,
    AgentSpec,
    ArtifactRef,
    FanoutBudget,
    RunContext,
    RunLimits,
    Usage,
)
from .registry import RunnerRegistry
from .resume import SwarmResumePolicy, resolve_resume_policy, resume_policy_from_value
from .runtime import SwarmRunResult, SwarmRuntime
from .state import FileSwarmStateStore, SwarmStateStore, agent_result_from_dict, swarm_run_result_to_dict
from .subprocess_runner import SubprocessCommand, SubprocessRunner, build_subprocess_registry
from .team import (
    FileTeamHandoffStore,
    TeamHandoff,
    TeamRunnerHandoff,
    build_team_handoff,
    task_for_team_handoff_resume,
    team_handoff_from_dict,
)
from .trace import SwarmTraceEvent, SwarmTraceRecorder

__all__ = [
    "AgentDescriptor",
    "AgentEvent",
    "AgentResult",
    "AgentRunHandle",
    "AgentRunner",
    "AgentSpec",
    "A2ARequestConfig",
    "A2ARunner",
    "ArtifactRef",
    "COORDINATOR_RECEIPT_FILE",
    "FanoutBudget",
    "FileSwarmStateStore",
    "FileTeamHandoffStore",
    "FunctionRunner",
    "HttpRequestConfig",
    "HttpRunner",
    "MergeApplyResult",
    "MergeApprovalDecision",
    "MergeApprovalPolicy",
    "MergeApprovalReason",
    "MergeApprovalStatus",
    "MergeChange",
    "MergeConflict",
    "MergePlan",
    "RunnerConfig",
    "RunnerRegistry",
    "RunContext",
    "RunLimits",
    "SubprocessCommand",
    "SubprocessRunner",
    "SwarmConfig",
    "SwarmCoordinatorOptions",
    "SwarmCoordinatorReceipt",
    "SwarmCoordinatorResult",
    "SwarmInspectionConfig",
    "SwarmLangfuseExportResult",
    "SwarmLangfuseExporter",
    "SwarmResumePolicy",
    "SwarmRunResult",
    "SwarmRuntime",
    "SwarmStateStore",
    "SwarmTraceEvent",
    "SwarmTraceRecorder",
    "TaskConfig",
    "TeamHandoff",
    "TeamRunnerHandoff",
    "Usage",
    "WorkerWorkspace",
    "WorkerWorkspaceConfig",
    "agent_result_from_dict",
    "build_a2a_registry",
    "build_function_registry",
    "build_http_registry",
    "build_merge_plan",
    "build_subprocess_registry",
    "build_team_handoff",
    "create_inspection_server",
    "evaluate_merge_plan",
    "export_swarm_trace_to_langfuse",
    "load_run_artifact",
    "load_run_detail",
    "load_run_index",
    "load_swarm_config",
    "apply_merge_plan",
    "merge_approval_policy_from_metadata",
    "merge_approval_policy_from_value",
    "prepare_worker_workspace",
    "resolve_resume_policy",
    "resolve_worker_workspace_config",
    "resume_policy_from_value",
    "run_swarm_coordinator",
    "serve_inspection_api",
    "swarm_run_result_to_dict",
    "task_for_team_handoff_resume",
    "team_handoff_from_dict",
    "write_coordinator_receipt",
]
