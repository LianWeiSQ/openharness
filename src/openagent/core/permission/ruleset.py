from __future__ import annotations

from dataclasses import dataclass
from enum import Enum

from .rule import PermissionAction, PermissionRule


class PermissionRuleset(str, Enum):
    FULL = "FULL"
    READONLY = "READONLY"
    PLAN_ONLY = "PLAN_ONLY"
    NONE = "NONE"


@dataclass(frozen=True, slots=True)
class PermissionRulesetDef:
    name: PermissionRuleset
    rules: list[PermissionRule]


def ruleset(name: PermissionRuleset) -> PermissionRulesetDef:
    if name == PermissionRuleset.FULL:
        return PermissionRulesetDef(
            name=name,
            rules=[
                PermissionRule(tool="*", action=PermissionAction.ALLOW, pattern="*"),
            ],
        )
    if name == PermissionRuleset.READONLY:
        return PermissionRulesetDef(
            name=name,
            rules=[
                # - PermissionManager 使用“last match wins”（最后匹配优先）
                # - 因此这里需要先给出默认 DENY，再在后面对白名单工具做 ALLOW 覆盖
                PermissionRule(tool="*", action=PermissionAction.DENY, pattern="*"),
                PermissionRule(tool="read", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="glob", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="grep", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="ls", action=PermissionAction.ALLOW, pattern="*"),
            ],
        )
    if name == PermissionRuleset.PLAN_ONLY:
        return PermissionRulesetDef(
            name=name,
            rules=[
                # PLAN_ONLY 默认 ASK（需要用户确认），但对白名单只读工具直接放行
                PermissionRule(tool="*", action=PermissionAction.ASK, pattern="*"),
                PermissionRule(tool="read", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="glob", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="grep", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="ls", action=PermissionAction.ALLOW, pattern="*"),
            ],
        )
    if name == PermissionRuleset.NONE:
        return PermissionRulesetDef(
            name=name,
            rules=[
                PermissionRule(tool="*", action=PermissionAction.DENY, pattern="*"),
            ],
        )
    raise ValueError(f"Unknown ruleset: {name}")
