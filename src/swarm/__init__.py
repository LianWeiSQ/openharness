from __future__ import annotations

"""Agent-agnostic swarm/function kernel.

The swarm package intentionally has no dependency on openagent. OpenAgent can
adapt into this kernel later, but the kernel should remain usable with plain
Python functions, subprocess agents, HTTP agents, or any other runner.
"""

from .config import RunnerConfig, SwarmConfig, TaskConfig, load_swarm_config
from .function_runner import FunctionRunner, build_function_registry
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
from .runtime import SwarmRunResult, SwarmRuntime
from .subprocess_runner import SubprocessCommand, SubprocessRunner, build_subprocess_registry
from .trace import SwarmTraceEvent, SwarmTraceRecorder

__all__ = [
    "AgentDescriptor",
    "AgentEvent",
    "AgentResult",
    "AgentRunHandle",
    "AgentRunner",
    "AgentSpec",
    "ArtifactRef",
    "FanoutBudget",
    "FunctionRunner",
    "RunnerConfig",
    "RunnerRegistry",
    "RunContext",
    "RunLimits",
    "SubprocessCommand",
    "SubprocessRunner",
    "SwarmConfig",
    "SwarmRunResult",
    "SwarmRuntime",
    "SwarmTraceEvent",
    "SwarmTraceRecorder",
    "TaskConfig",
    "Usage",
    "build_function_registry",
    "build_subprocess_registry",
    "load_swarm_config",
]
