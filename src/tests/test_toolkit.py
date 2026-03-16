from __future__ import annotations

import shutil
import unittest
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

from openagent.core.permission.manager import PermissionDeniedError
from openagent.core.permission.manager import PermissionManager
from openagent.core.permission.ruleset import PermissionRuleset
from openagent.core.tool.definition import ToolContext, ToolOutput
from openagent.core.tool.middleware import permission_middleware
from openagent.core.tool.toolkit import ToolkitAdapter
from openagent.core.tool.truncation import Truncate


@dataclass
class NoArgs:
    pass


class ToolkitTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_root(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = (tmp_root / f"t_{uuid4().hex}").resolve()
        td.mkdir(parents=True, exist_ok=True)
        return td

    async def test_write_denied_in_readonly(self) -> None:
        pm = PermissionManager()
        pm.set_ruleset(PermissionRuleset.READONLY)
        tk = ToolkitAdapter()
        tk.register_middleware(permission_middleware(pm))
        tk.load_builtin()
        td = self._make_temp_root()
        try:
            with self.assertRaises(PermissionDeniedError):
                await tk.execute(
                    name="write",
                    input={"file_path": "x.txt", "content": "hello"},
                    context={"session_root": str(td)},
                )
        finally:
            shutil.rmtree(td, ignore_errors=True)

    async def test_register_tool_compat_shim_keeps_schema_and_executes(self) -> None:
        tk = ToolkitAdapter()

        async def ping(params: dict[str, str], _ctx: dict[str, str]) -> str:
            return f"pong:{params['value']}"

        schema = {
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
        }
        tk.register_tool("legacy_ping", ping, description="legacy", schema=schema, group="legacy")

        tools = {tool.name: tool for tool in tk.get_all_tools()}
        self.assertIn("legacy_ping", tools)
        self.assertEqual(tools["legacy_ping"].schema, schema)
        self.assertEqual(tools["legacy_ping"].group, "legacy")

        res = await tk.execute(name="legacy_ping", input={"value": "ok"}, context={})
        self.assertIsNone(res.error)
        self.assertEqual(res.output, "pong:ok")

    async def test_register_mcp_is_explicitly_not_implemented(self) -> None:
        tk = ToolkitAdapter()
        with self.assertRaises(NotImplementedError):
            tk.register_mcp(object())

    async def test_tool_semantic_truncation_is_preserved(self) -> None:
        tk = ToolkitAdapter()
        td = self._make_temp_root()
        try:
            async def run(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
                return ToolOutput(title="semantic", output="partial", metadata={"count": 3}, truncated=True)

            tk.registry.define_tool(tool_id="semantic", parameters=NoArgs, description="# semantic")(run)
            res = await tk.execute(name="semantic", input={}, context={"session_root": str(td)})

            self.assertIsNone(res.error)
            self.assertEqual(res.output, "partial")
            self.assertTrue(res.metadata["truncated"])
            self.assertFalse(res.metadata["output_truncated"])
            self.assertNotIn("output_path", res.metadata)
        finally:
            shutil.rmtree(td, ignore_errors=True)

    async def test_output_truncation_writes_full_output(self) -> None:
        tk = ToolkitAdapter()
        td = self._make_temp_root()
        try:
            big_output = "x" * (Truncate.DEFAULT_MAX_BYTES + 32)

            async def run(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
                return ToolOutput(title="big", output=big_output)

            tk.registry.define_tool(tool_id="big", parameters=NoArgs, description="# big")(run)
            res = await tk.execute(name="big", input={}, context={"session_root": str(td)})

            self.assertIsNone(res.error)
            self.assertTrue(res.metadata["truncated"])
            self.assertTrue(res.metadata["output_truncated"])
            output_path = Path(res.metadata["output_path"])
            self.assertTrue(output_path.exists())
            self.assertEqual(output_path.read_text(encoding="utf-8"), big_output)
        finally:
            shutil.rmtree(td, ignore_errors=True)

    async def test_semantic_and_output_truncation_do_not_override_each_other(self) -> None:
        tk = ToolkitAdapter()
        td = self._make_temp_root()
        try:
            big_output = "y" * (Truncate.DEFAULT_MAX_BYTES + 64)

            async def run(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
                return ToolOutput(title="both", output=big_output, truncated=True)

            tk.registry.define_tool(tool_id="both", parameters=NoArgs, description="# both")(run)
            res = await tk.execute(name="both", input={}, context={"session_root": str(td)})

            self.assertIsNone(res.error)
            self.assertTrue(res.metadata["truncated"])
            self.assertTrue(res.metadata["output_truncated"])
            self.assertIn("output_path", res.metadata)
        finally:
            shutil.rmtree(td, ignore_errors=True)
