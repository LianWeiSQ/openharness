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
                PermissionRule(tool="*", action=PermissionAction.DENY, pattern="*"),
                PermissionRule(tool="read", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="glob", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="grep", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="ls", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="skill", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="todoread", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="question", action=PermissionAction.ALLOW, pattern="*"),
            ],
        )
    if name == PermissionRuleset.PLAN_ONLY:
        return PermissionRulesetDef(
            name=name,
            rules=[
                PermissionRule(tool="*", action=PermissionAction.ASK, pattern="*"),
                PermissionRule(tool="read", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="glob", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="grep", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="ls", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="skill", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="todoread", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="todowrite", action=PermissionAction.ALLOW, pattern="*"),
                PermissionRule(tool="question", action=PermissionAction.ALLOW, pattern="*"),
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

