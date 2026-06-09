from __future__ import annotations

"""
PermissionManager：权限决策器（allow / deny / ask）。

核心目标：
- 在工具执行前，根据规则集对工具调用进行“可用性判定”
- 支持三态：ALLOW（直接允许）、DENY（直接拒绝）、ASK（需要用户确认）

实现要点：
- 规则匹配使用 fnmatch（支持通配符），并采用“最后匹配优先（last match wins）”
- ask_user_func 由上层注入，用于 UI/交互式确认；如果未注入则抛出 PermissionAskRequiredError
"""

import fnmatch
import json
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from typing import Any

from .rule import PermissionAction, PermissionRule
from .ruleset import PermissionRuleset, ruleset as builtin_ruleset


class PermissionDeniedError(RuntimeError):
    pass


class PermissionAskRequiredError(RuntimeError):
    pass


AskUserFunc = Callable[[dict[str, Any]], Awaitable[PermissionAction]]


@dataclass(slots=True)
class PermissionManager:
    ask_user_func: AskUserFunc | None = None
    _rules: list[PermissionRule] | None = None

    def set_ruleset(self, name: PermissionRuleset) -> None:
        # 直接替换当前规则集（内置规则集见 ruleset.py）
        self._rules = list(builtin_ruleset(name).rules)

    def add_rule(self, rule: PermissionRule) -> None:
        # 追加自定义规则（注意：last match wins，因此后加规则优先级更高）
        if self._rules is None:
            self._rules = []
        self._rules.append(rule)

    def _evaluate(self, tool: str, pattern: str) -> PermissionRule | None:
        rules = self._rules or []
        match: PermissionRule | None = None
        for rule in rules:
            if fnmatch.fnmatch(tool, rule.tool) and (rule.pattern is None or fnmatch.fnmatch(pattern, rule.pattern)):
                match = rule
        return match

    async def check(self, tool_call: dict[str, Any]) -> PermissionAction:
        # tool_call 最小结构：{"name": "...", "input": {...}}
        tool = str(tool_call.get("name") or tool_call.get("tool") or "")
        payload = tool_call.get("input") or {}
        pattern = self._pattern_for(payload)
        rule = self._evaluate(tool, pattern) or PermissionRule(tool=tool, action=PermissionAction.ASK, pattern="*")
        if rule.condition and not rule.condition(tool_call):
            return PermissionAction.DENY
        if rule.action == PermissionAction.ASK:
            return await self.ask_user(tool_call)
        return rule.action

    async def ask_user(self, tool_call: dict[str, Any]) -> PermissionAction:
        # 未注入 ask_user_func 时，无法与用户交互确认，直接抛错
        if not self.ask_user_func:
            raise PermissionAskRequiredError(f"Permission requires user confirmation: {tool_call.get('name')}")
        return await self.ask_user_func(tool_call)

    @staticmethod
    def _pattern_for(payload: Any) -> str:
        # 尝试提取一个“可匹配的 pattern”：
        # - 文件类工具优先用 file_path / path
        # - shell 类工具优先用 command
        if isinstance(payload, dict):
            for key in ("file_path", "filePath", "path", "pattern", "command", "name"):
                v = payload.get(key)
                if isinstance(v, str) and v:
                    return v
        try:
            return json.dumps(payload, sort_keys=True, ensure_ascii=False)
        except Exception:
            return str(payload)
