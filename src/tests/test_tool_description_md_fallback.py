from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.tool.registry import ToolRegistry


class ToolDescriptionMdFallbackTests(unittest.TestCase):
    def test_description_md_used_when_exists(self) -> None:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = (tmp_root / f"t_{uuid4().hex}").resolve()
        td.mkdir(parents=True, exist_ok=True)
        try:
            plugin = td / "foo.py"
            md = td / "foo.md"
            md.write_text("# Foo\n\n来自 Markdown。", encoding="utf-8")
            plugin.write_text(
                "\n".join(
                    [
                        "from __future__ import annotations",
                        "from dataclasses import dataclass",
                        "from openagent.core.tool.definition import ToolContext, ToolOutput",
                        "from openagent.core.tool.registry import ToolRegistry",
                        "",
                        "@dataclass",
                        "class Params:",
                        "    pass",
                        "",
                        "async def run(_args: Params, _ctx: ToolContext) -> ToolOutput:",
                        "    return ToolOutput(title='t', output='ok', metadata={})",
                        "",
                        "def register(registry: ToolRegistry) -> None:",
                        "    registry.define_tool(tool_id='default', parameters=Params, description_md='foo.md', group='x')(run)",
                        "",
                    ]
                ),
                encoding="utf-8",
            )

            reg = ToolRegistry()
            reg.load_plugins(tool_paths=[str(td)], base_dir=td)
            tool = reg.get("foo")
            self.assertIsNotNone(tool)
            self.assertEqual(tool.description, "# Foo\n\n来自 Markdown。")
        finally:
            shutil.rmtree(td, ignore_errors=True)

    def test_description_string_used_when_md_missing(self) -> None:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = (tmp_root / f"t_{uuid4().hex}").resolve()
        td.mkdir(parents=True, exist_ok=True)
        try:
            plugin = td / "bar.py"
            plugin.write_text(
                "\n".join(
                    [
                        "from __future__ import annotations",
                        "from dataclasses import dataclass",
                        "from openagent.core.tool.definition import ToolContext, ToolOutput",
                        "from openagent.core.tool.registry import ToolRegistry",
                        "",
                        "@dataclass",
                        "class Params:",
                        "    pass",
                        "",
                        "async def run(_args: Params, _ctx: ToolContext) -> ToolOutput:",
                        "    return ToolOutput(title='t', output='ok', metadata={})",
                        "",
                        "def register(registry: ToolRegistry) -> None:",
                        "    registry.define_tool(tool_id='default', parameters=Params, description_md='missing.md', description='**Bar**')(run)",
                        "",
                    ]
                ),
                encoding="utf-8",
            )

            reg = ToolRegistry()
            reg.load_plugins(tool_paths=[str(td)], base_dir=td)
            tool = reg.get("bar")
            self.assertIsNotNone(tool)
            self.assertEqual(tool.description, "**Bar**")
        finally:
            shutil.rmtree(td, ignore_errors=True)

    def test_raises_when_no_md_and_no_description(self) -> None:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = (tmp_root / f"t_{uuid4().hex}").resolve()
        td.mkdir(parents=True, exist_ok=True)
        try:
            plugin = td / "baz.py"
            plugin.write_text(
                "\n".join(
                    [
                        "from __future__ import annotations",
                        "from dataclasses import dataclass",
                        "from openagent.core.tool.definition import ToolContext, ToolOutput",
                        "from openagent.core.tool.registry import ToolRegistry",
                        "",
                        "@dataclass",
                        "class Params:",
                        "    pass",
                        "",
                        "async def run(_args: Params, _ctx: ToolContext) -> ToolOutput:",
                        "    return ToolOutput(title='t', output='ok', metadata={})",
                        "",
                        "def register(registry: ToolRegistry) -> None:",
                        "    registry.define_tool(tool_id='default', parameters=Params, description_md='missing.md')(run)",
                        "",
                    ]
                ),
                encoding="utf-8",
            )

            reg = ToolRegistry()
            with self.assertRaises(ValueError):
                reg.load_plugins(tool_paths=[str(td)], base_dir=td)
        finally:
            shutil.rmtree(td, ignore_errors=True)

