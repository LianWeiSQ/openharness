from __future__ import annotations

import unittest

from openagent.core.permission.manager import PermissionManager
from openagent.core.permission.rule import PermissionAction
from openagent.core.permission.ruleset import PermissionRuleset


class PermissionTests(unittest.IsolatedAsyncioTestCase):
    async def test_readonly_denies_write(self) -> None:
        pm = PermissionManager()
        pm.set_ruleset(PermissionRuleset.READONLY)
        action = await pm.check({"name": "write", "input": {"file_path": "a.txt", "content": "x"}})
        self.assertEqual(action, PermissionAction.DENY)

    async def test_readonly_allows_ls(self) -> None:
        pm = PermissionManager()
        pm.set_ruleset(PermissionRuleset.READONLY)
        action = await pm.check({"name": "ls", "input": {}})
        self.assertEqual(action, PermissionAction.ALLOW)

    async def test_readonly_allows_todoread_and_denies_todowrite(self) -> None:
        pm = PermissionManager()
        pm.set_ruleset(PermissionRuleset.READONLY)
        self.assertEqual(await pm.check({"name": "todoread", "input": {}}), PermissionAction.ALLOW)
        self.assertEqual(await pm.check({"name": "todowrite", "input": {"todos": []}}), PermissionAction.DENY)

    async def test_plan_only_allows_todoread_and_todowrite(self) -> None:
        pm = PermissionManager()
        pm.set_ruleset(PermissionRuleset.PLAN_ONLY)
        self.assertEqual(await pm.check({"name": "todoread", "input": {}}), PermissionAction.ALLOW)
        self.assertEqual(await pm.check({"name": "todowrite", "input": {"todos": []}}), PermissionAction.ALLOW)

    async def test_full_allows_bash(self) -> None:
        pm = PermissionManager()
        pm.set_ruleset(PermissionRuleset.FULL)
        action = await pm.check({"name": "bash", "input": {"command": "echo hi"}})
        self.assertEqual(action, PermissionAction.ALLOW)
