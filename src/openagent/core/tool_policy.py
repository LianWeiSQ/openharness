from __future__ import annotations

from dataclasses import dataclass

from .types import ChatMessage

NO_TOOL_PATTERNS = ("不要调用工具", "不要用工具", "don't use tools", "do not use tools", "without tools")
RESEARCH_STRONG_PATTERNS = (
    "调研",
    "综述",
    "文献",
    "research",
    "survey",
    "literature",
    "evidence",
)
RESEARCH_WEAK_PATTERNS = (
    "研究",
    "分析",
    "比较",
    "新型材料",
    "investigate",
    "analyze",
    "analysis",
    "compare",
)
CURRENT_PATTERNS = (
    "最新",
    "当前",
    "现在",
    "今天",
    "近期",
    "最近",
    "纪录",
    "记录",
    "价格",
    "政策",
    "latest",
    "current",
    "today",
    "recent",
    "news",
    "price",
    "record",
    "up to date",
    "up-to-date",
)
PLAN_STRONG_PATTERNS = (
    "完整方案",
    "实验方案",
    "路线图",
    "分阶段",
    "设计一套",
    "实施方案",
    "排查方案",
    "protocol",
    "workflow",
    "roadmap",
    "step-by-step",
    "full plan",
    "complete plan",
    "experimental plan",
    "experiment plan",
    "implementation plan",
)
PLAN_WEAK_PATTERNS = (
    "方案",
    "计划",
    "设计一个",
    "plan",
)
CLARIFICATION_PATTERNS = (
    "请提供",
    "请说明",
    "请问",
    "哪种",
    "哪个",
    "哪一个",
    "which ",
    "what ",
    "please clarify",
    "could you clarify",
)


@dataclass(frozen=True, slots=True)
class ToolCapability:
    name: str
    tools: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class ToolPolicy:
    scenario: str
    required_capabilities: tuple[ToolCapability, ...]
    acceptable_first_tools: tuple[str, ...]
    reminder: str

    @property
    def required_tools(self) -> tuple[str, ...]:
        ordered: list[str] = []
        for capability in self.required_capabilities:
            for tool_name in capability.tools:
                if tool_name not in ordered:
                    ordered.append(tool_name)
        return tuple(ordered)


RESEARCH_CAPABILITY = ToolCapability(name="research", tools=("web_search",))
PLANNING_CAPABILITY = ToolCapability(name="planning", tools=("todoread", "todowrite"))


RESEARCH_POLICY = ToolPolicy(
    scenario="research",
    required_capabilities=(RESEARCH_CAPABILITY,),
    acceptable_first_tools=("web_search",),
    reminder=(
        "Tool policy reminder: this request requires external research. Do not answer from memory yet. "
        "Use `web_search` first, then use `web_fetch` when you need source content or evidence."
    ),
)
CURRENT_POLICY = ToolPolicy(
    scenario="current",
    required_capabilities=(RESEARCH_CAPABILITY,),
    acceptable_first_tools=("web_search",),
    reminder=(
        "Tool policy reminder: this request is time-sensitive or asks for current information. "
        "Use `web_search` first, then use `web_fetch` if you need source content or details."
    ),
)
PLAN_POLICY = ToolPolicy(
    scenario="plan",
    required_capabilities=(PLANNING_CAPABILITY,),
    acceptable_first_tools=("todoread", "todowrite"),
    reminder=(
        "Tool policy reminder: this request needs a visible multi-step plan. "
        "Use `todoread` or `todowrite` early so the work stays grounded in a visible task list before giving the full plan."
    ),
)
RESEARCH_PLAN_POLICY = ToolPolicy(
    scenario="research_plan",
    required_capabilities=(PLANNING_CAPABILITY, RESEARCH_CAPABILITY),
    acceptable_first_tools=("todoread", "todowrite", "web_search"),
    reminder=(
        "Tool policy reminder: this request combines multi-step planning with research. "
        "Before the final answer, cover both planning and research with tools. "
        "Use `todoread` or `todowrite` for planning, use `web_search` for research, "
        "and use `web_fetch` after search when you need sources or evidence."
    ),
)


def classify_tool_policy(user_text: str) -> ToolPolicy | None:
    text = user_text.strip()
    if not text:
        return None

    lowered = text.lower()
    if any(pattern in lowered for pattern in NO_TOOL_PATTERNS):
        return None

    research_strong = _contains_any(text, lowered, RESEARCH_STRONG_PATTERNS)
    is_research = research_strong or _contains_any(text, lowered, RESEARCH_WEAK_PATTERNS)
    is_current = _contains_any(text, lowered, CURRENT_PATTERNS)
    plan_strong = _contains_any(text, lowered, PLAN_STRONG_PATTERNS)
    is_plan = plan_strong or _contains_any(text, lowered, PLAN_WEAK_PATTERNS)

    if (research_strong or is_current) and plan_strong:
        return RESEARCH_PLAN_POLICY
    if is_current:
        return CURRENT_POLICY
    if is_plan:
        return PLAN_POLICY
    if is_research:
        return RESEARCH_POLICY
    return None


def missing_required_tools(policy: ToolPolicy, tool_names: set[str]) -> list[str]:
    missing: list[str] = []
    for capability in policy.required_capabilities:
        if any(tool_name in tool_names for tool_name in capability.tools):
            continue
        for tool_name in capability.tools:
            if tool_name not in missing:
                missing.append(tool_name)
    return missing


def missing_required_capabilities(policy: ToolPolicy, tool_names: set[str]) -> list[ToolCapability]:
    return [
        capability
        for capability in policy.required_capabilities
        if not any(tool_name in tool_names for tool_name in capability.tools)
    ]


def actionable_missing_capabilities(
    policy: ToolPolicy,
    tool_names: set[str],
    *,
    available_tools: set[str] | None = None,
    failed_tools: set[str] | None = None,
) -> list[ToolCapability]:
    available = available_tools or set(policy.required_tools)
    failed = failed_tools or set()
    actionable: list[ToolCapability] = []
    for capability in missing_required_capabilities(policy, tool_names):
        available_options = [tool_name for tool_name in capability.tools if tool_name in available]
        if not available_options:
            continue
        if all(tool_name in failed for tool_name in available_options):
            continue
        actionable.append(capability)
    return actionable


def format_missing_tools_error(policy: ToolPolicy, missing_tools: list[str]) -> str:
    missing = ", ".join(missing_tools)
    required = ", ".join(policy.required_tools)
    return (
        f"Tool policy requires unavailable tools for this request: scenario={policy.scenario}, "
        f"missing={missing}, required={required}"
    )


def format_tool_policy_retry_error(
    policy: ToolPolicy,
    missing_capabilities: list[ToolCapability] | None = None,
) -> str:
    requirements = missing_capabilities or list(policy.required_capabilities)
    required = ", ".join(_format_capability_requirement(capability) for capability in requirements)
    return (
        f"Tool policy requires tool use before answering this request: scenario={policy.scenario}, "
        f"required={required}"
    )


def should_accept_tool_calls(policy: ToolPolicy, tool_names: list[str]) -> bool:
    if not tool_names:
        return False
    return any(tool_name in policy.acceptable_first_tools for tool_name in tool_names)


def format_tool_policy_reminder(
    policy: ToolPolicy,
    missing_capabilities: list[ToolCapability] | None = None,
) -> str:
    requirements = missing_capabilities or list(policy.required_capabilities)
    formatted = ", ".join(_format_capability_requirement(capability) for capability in requirements)
    return f"{policy.reminder} Missing capability gaps: {formatted}."


def recent_failed_required_tools(
    messages: list[ChatMessage],
    policy: ToolPolicy,
    *,
    lookback_user_turns: int = 1,
) -> set[str]:
    if lookback_user_turns <= 0 or not messages or not policy.required_capabilities:
        return set()

    latest_user_index = next((index for index in range(len(messages) - 1, -1, -1) if messages[index].role == "user"), None)
    if latest_user_index is None or latest_user_index <= 0:
        return set()

    start_index = 0
    remaining_turns = lookback_user_turns
    for index in range(latest_user_index - 1, -1, -1):
        if messages[index].role != "user":
            continue
        remaining_turns -= 1
        if remaining_turns == 0:
            start_index = index
            break

    failed_tools: set[str] = set()
    for message in messages[start_index:latest_user_index]:
        if message.role != "tool":
            continue
        metadata = message.metadata or {}
        raw_tool_name = metadata.get("tool")
        tool_name = raw_tool_name if isinstance(raw_tool_name, str) and raw_tool_name else (message.name or "")
        if tool_name not in policy.required_tools:
            continue

        error_kind = metadata.get("error_kind")
        if isinstance(error_kind, str) and error_kind:
            failed_tools.add(tool_name)
            continue
        if "status=error" in message.content:
            failed_tools.add(tool_name)

    return failed_tools


def looks_like_clarification_request(text: str) -> bool:
    stripped = text.strip()
    if not stripped:
        return False
    lowered = stripped.lower()
    if stripped.endswith("?") or stripped.endswith("？"):
        return True
    return any(pattern in lowered for pattern in CLARIFICATION_PATTERNS)


def _contains_any(original: str, lowered: str, patterns: tuple[str, ...]) -> bool:
    for pattern in patterns:
        if pattern.lower() in lowered or pattern in original:
            return True
    return False


def _format_capability_requirement(capability: ToolCapability) -> str:
    if len(capability.tools) == 1:
        return capability.tools[0]
    return f"{capability.name}(" + "|".join(capability.tools) + ")"


__all__ = [
    "ToolCapability",
    "ToolPolicy",
    "actionable_missing_capabilities",
    "classify_tool_policy",
    "format_missing_tools_error",
    "format_tool_policy_reminder",
    "format_tool_policy_retry_error",
    "looks_like_clarification_request",
    "missing_required_capabilities",
    "missing_required_tools",
    "recent_failed_required_tools",
    "should_accept_tool_calls",
]
