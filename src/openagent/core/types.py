from __future__ import annotations

"""
核心类型定义。

该文件承载 OpenAgent 内部协议与跨模块共享的数据结构：
- Model / ChatMessage：对齐 Provider 侧需要的最小对话结构
- ToolCall / ToolResult：对齐 AgentLoop 与 Toolkit 的工具调用协议
- StreamEvent：对齐 `Agent.md` 的“流事件类型”表（text/tool/step/error/patch）

设计原则：
- 让 Loop/Tool/Permission/Provider 之间通过类型解耦
- 避免对第三方 SDK 的强绑定（便于替换 Provider）
"""

import json
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Awaitable, Callable, Literal, TypedDict

JsonPrimitive = str | int | float | bool | None
JsonValue = JsonPrimitive | list["JsonValue"] | dict[str, "JsonValue"]

Role = Literal["system", "user", "assistant", "tool"]
FinishReason = Literal["stop", "tool_call", "length", "error", "unknown"]
PermissionRulesetName = Literal["FULL", "READONLY", "PLAN_ONLY", "NONE"]


@dataclass(frozen=True, slots=True)
class ModelPricing:
    input_per_1m: float = 0.0
    output_per_1m: float = 0.0


@dataclass(frozen=True, slots=True)
class ModelCapabilities:
    vision: bool = False
    tools: bool = True
    streaming: bool = True
    reasoning: bool = False


@dataclass(frozen=True, slots=True)
class Model:
    id: str
    provider_id: str
    name: str
    context_window: int
    max_output: int
    capabilities: ModelCapabilities = field(default_factory=ModelCapabilities)
    pricing: ModelPricing = field(default_factory=ModelPricing)


@dataclass(frozen=True, slots=True)
class ChatMessage:
    role: Role
    content: str
    name: str | None = None
    tool_call_id: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class ToolSchema:
    name: str
    description: str = ""
    schema: dict[str, Any] | None = None
    group: str = "default"
    dangerous: bool = False


@dataclass(frozen=True, slots=True)
class ToolCall:
    name: str
    input: dict[str, Any]
    call_id: str

    def key(self) -> str:
        return f"{self.name}:{json.dumps(self.input, sort_keys=True, ensure_ascii=False)}"


@dataclass(frozen=True, slots=True)
class ToolResult:
    call_id: str
    output: str
    error: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class Usage:
    input_tokens: int = 0
    output_tokens: int = 0
    cost: float = 0.0


@dataclass(slots=True)
class AgentConfig:
    name: str
    mode: Literal["primary", "subagent"] = "primary"
    prompt: str | None = None
    model: Model | None = None
    tools: list[str] | Literal["all", "readonly"] = "all"
    permission: PermissionRulesetName = "FULL"
    max_steps: int = 50
    temperature: float | None = None
    options: dict[str, Any] = field(default_factory=dict)


class SessionStatus(str, Enum):
    IDLE = "idle"
    RUNNING = "running"
    PAUSED = "paused"
    STOP = "stop"
    COMPACTING = "compacting"


class TextStartEvent(TypedDict):
    type: Literal["text-start"]
    id: str
    metadata: dict[str, Any] | None


class TextDeltaEvent(TypedDict):
    type: Literal["text-delta"]
    id: str
    text: str


class TextEndEvent(TypedDict):
    type: Literal["text-end"]
    id: str


class ToolCallEvent(TypedDict):
    type: Literal["tool-call"]
    name: str
    input: dict[str, Any]
    call_id: str


class ToolResultEvent(TypedDict):
    type: Literal["tool-result"]
    call_id: str
    output: str
    error: str | None
    metadata: dict[str, Any] | None


class StepStartEvent(TypedDict):
    type: Literal["step-start"]
    snapshot_id: str


class StepFinishEvent(TypedDict):
    type: Literal["step-finish"]
    tokens: dict[str, int]
    cost: float
    finish_reason: FinishReason


class ErrorEvent(TypedDict):
    type: Literal["error"]
    error: str


class PatchEvent(TypedDict, total=False):
    # Not listed in Agent.md event table, but required by the loop flow (file snapshot → patch).
    type: Literal["patch"]
    snapshot_id: str
    hash: str
    files: list[dict[str, Any]]


StreamEvent = (
    TextStartEvent
    | TextDeltaEvent
    | TextEndEvent
    | ToolCallEvent
    | ToolResultEvent
    | StepStartEvent
    | StepFinishEvent
    | PatchEvent
    | ErrorEvent
)


MiddlewareFunc = Callable[
    [ToolCall, Callable[[ToolCall], Awaitable[ToolResult]], dict[str, Any]],
    Awaitable[ToolResult],
]
