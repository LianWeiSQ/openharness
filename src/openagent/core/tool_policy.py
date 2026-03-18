from __future__ import annotations

from dataclasses import dataclass

NO_TOOL_PATTERNS = ("不要调用工具", "不要用工具", "don\'t use tools", "do not use tools", "without tools")
RESEARCH_PATTERNS = (
    "研究",
    "调研",
    "综述",
    "文献",
    "分析",
    "比较",
    "新型材料",
    "research",
    "investigate",
    "survey",
    "literature",
    "evidence",
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
PLAN_PATTERNS = (
    "完整方案",
    "实验方案",
    "方案",
    "路线图",
    "计划",
    "分阶段",
    "设计一套",
    "设计一个",
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
class ToolPolicy:
    scenario: str
    required_tools: tuple[str, ...]
    first_tool_options: tuple[str, ...]
    reminder: str


RESEARCH_POLICY = ToolPolicy(
    scenario="research",
    required_tools=("web_search",),
    first_tool_options=("web_search",),
    reminder=(
        "Tool policy reminder: this request requires external research. Do not answer from memory yet. "
        "Use `web_search` first, then use `web_fetch` when you need source content or evidence."
    ),
)
CURRENT_POLICY = ToolPolicy(
    scenario="current",
    required_tools=("web_search",),
    first_tool_options=("web_search",),
    reminder=(
        "Tool policy reminder: this request is time-sensitive or asks for current information. "
        "Use `web_search` first, then use `web_fetch` if you need source content or details."
    ),
)
PLAN_POLICY = ToolPolicy(
    scenario="plan",
    required_tools=("todoread", "todowrite"),
    first_tool_options=("todoread",),
    reminder=(
        "Tool policy reminder: this request needs a visible multi-step plan. "
        "Use `todoread` first, then `todowrite` to create or update the structured task list before giving the full plan."
    ),
)
RESEARCH_PLAN_POLICY = ToolPolicy(
    scenario="research_plan",
    required_tools=("todoread", "todowrite", "web_search"),
    first_tool_options=("todoread",),
    reminder=(
        "Tool policy reminder: this request combines multi-step planning with research. "
        "Use `todoread` first, then `todowrite`, then `web_search`, and use `web_fetch` after search when you need sources or evidence."
    ),
)


def classify_tool_policy(user_text: str) -> ToolPolicy | None:
    text = user_text.strip()
    if not text:
        return None

    lowered = text.lower()
    if any(pattern in lowered for pattern in NO_TOOL_PATTERNS):
        return None

    is_research = _contains_any(text, lowered, RESEARCH_PATTERNS)
    is_current = _contains_any(text, lowered, CURRENT_PATTERNS)
    is_plan = _contains_any(text, lowered, PLAN_PATTERNS)

    if (is_research or is_current) and is_plan:
        return RESEARCH_PLAN_POLICY
    if is_current:
        return CURRENT_POLICY
    if is_research:
        return RESEARCH_POLICY
    if is_plan:
        return PLAN_POLICY
    return None


def missing_required_tools(policy: ToolPolicy, tool_names: set[str]) -> list[str]:
    return [tool_name for tool_name in policy.required_tools if tool_name not in tool_names]


def format_missing_tools_error(policy: ToolPolicy, missing_tools: list[str]) -> str:
    missing = ", ".join(missing_tools)
    required = ", ".join(policy.required_tools)
    return (
        f"Tool policy requires unavailable tools for this request: scenario={policy.scenario}, "
        f"missing={missing}, required={required}"
    )


def format_tool_policy_retry_error(policy: ToolPolicy) -> str:
    required = ", ".join(policy.required_tools)
    return (
        f"Tool policy requires tool use before answering this request: scenario={policy.scenario}, "
        f"required={required}"
    )


def should_accept_tool_calls(policy: ToolPolicy, tool_names: list[str]) -> bool:
    if not tool_names:
        return False
    return any(tool_name in policy.first_tool_options for tool_name in tool_names)


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


__all__ = [
    "ToolPolicy",
    "classify_tool_policy",
    "format_missing_tools_error",
    "format_tool_policy_retry_error",
    "looks_like_clarification_request",
    "missing_required_tools",
    "should_accept_tool_calls",
]
