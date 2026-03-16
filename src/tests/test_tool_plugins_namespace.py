from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.tool.toolkit import ToolkitAdapter


class ToolPluginNamespaceTests(unittest.IsolatedAsyncioTestCase):
    async def test_plugin_ids_are_namespaced_and_executable(self) -> None:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = (tmp_root / f"t_{uuid4().hex}").resolve()
        td.mkdir(parents=True, exist_ok=True)
        try:
            # 目录下有 tool/ 子目录时，应优先扫描 tool/*.py（贴合 opencode）
            tool_dir = td / "tool"
            tool_dir.mkdir(parents=True, exist_ok=True)

            plugin = tool_dir / "hello.py"
            plugin.write_text(
                "\n".join(
                    [
                        "from __future__ import annotations",
                        "from dataclasses import dataclass",
                        "from openagent.core.tool.definition import ToolContext, ToolOutput",
                        "from openagent.core.tool.registry import ToolRegistry",
                        "",
                        "@dataclass",
                        "class NoArgs:",
                        "    pass",
                        "",
                        "async def t_default(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:",
                        "    return ToolOutput(title='t', output='DEFAULT', metadata={})",
                        "",
                        "async def t_bar(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:",
                        "    return ToolOutput(title='t', output='BAR', metadata={})",
                        "",
                        "def register(registry: ToolRegistry) -> None:",
                        "    registry.define_tool(tool_id='default', parameters=NoArgs, description='# default')(t_default)",
                        "    registry.define_tool(tool_id='bar', parameters=NoArgs, description='# bar')(t_bar)",
                        "",
                    ]
                ),
                encoding="utf-8",
            )

            toolkit = ToolkitAdapter()
            toolkit.load_plugins(tool_paths=[str(td)], base_dir=td)

            tool_names = {t.name for t in toolkit.get_all_tools()}
            self.assertIn("hello", tool_names)
            self.assertIn("hello_bar", tool_names)

            res = await toolkit.execute(name="hello", input={}, context={"session_root": str(td)})
            self.assertIsNone(res.error)
            self.assertEqual(res.output, "DEFAULT")

            res = await toolkit.execute(name="hello_bar", input={}, context={"session_root": str(td)})
            self.assertIsNone(res.error)
            self.assertEqual(res.output, "BAR")
        finally:
            shutil.rmtree(td, ignore_errors=True)

