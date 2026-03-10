from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.permission.manager import PermissionDeniedError
from openagent.core.permission.manager import PermissionManager
from openagent.core.permission.ruleset import PermissionRuleset
from openagent.core.tool.builtin.file import register_file_tools
from openagent.core.tool.middleware import permission_middleware
from openagent.core.tool.toolkit import ToolkitAdapter


class ToolkitTests(unittest.IsolatedAsyncioTestCase):
    async def test_write_denied_in_readonly(self) -> None:
        pm = PermissionManager()
        pm.set_ruleset(PermissionRuleset.READONLY)
        tk = ToolkitAdapter()
        tk.register_middleware(permission_middleware(pm))
        register_file_tools(tk)
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"t_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        try:
            with self.assertRaises(PermissionDeniedError):
                await tk.execute(
                    name="write",
                    input={"file_path": "x.txt", "content": "hello"},
                    context={"session_root": str(td)},
                )
        finally:
            shutil.rmtree(td, ignore_errors=True)
