from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.tool.toolkit import ToolkitAdapter


class SearchToolTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_root(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        root = (tmp_root / f"t_{uuid4().hex}").resolve()
        root.mkdir(parents=True, exist_ok=True)
        return root

    async def test_code_search_hits_and_miss(self) -> None:
        root = self._make_temp_root()
        try:
            (root / "a.py").write_text("alpha\nbeta\n", encoding="utf-8")
            (root / "b.txt").write_text("gamma\n", encoding="utf-8")

            toolkit = ToolkitAdapter()
            toolkit.load_builtin()
            ctx = {"session_root": str(root)}

            res = await toolkit.execute(
                name="code_search",
                input={"query": "beta", "glob": "*.py"},
                context=ctx,
            )
            self.assertIsNone(res.error)
            self.assertIn("a.py:2:beta", res.output)
            self.assertFalse(res.metadata["truncated"])
            self.assertFalse(res.metadata["output_truncated"])

            res = await toolkit.execute(
                name="code_search",
                input={"query": "nope", "glob": "*.py"},
                context=ctx,
            )
            self.assertIsNone(res.error)
            self.assertEqual(res.output, "")
            self.assertFalse(res.metadata["truncated"])
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_code_search_marks_semantic_truncation_at_hit_limit(self) -> None:
        root = self._make_temp_root()
        try:
            lines = [f"beta {i}" for i in range(210)]
            (root / "a.py").write_text("\n".join(lines), encoding="utf-8")

            toolkit = ToolkitAdapter()
            toolkit.load_builtin()
            res = await toolkit.execute(
                name="code_search",
                input={"query": "beta", "glob": "*.py"},
                context={"session_root": str(root)},
            )

            self.assertIsNone(res.error)
            self.assertTrue(res.metadata["truncated"])
            self.assertFalse(res.metadata["output_truncated"])
            self.assertEqual(res.metadata["count"], 200)
            self.assertEqual(len([line for line in res.output.splitlines() if line.strip()]), 200)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_grep_uses_regex_while_code_search_uses_substring(self) -> None:
        root = self._make_temp_root()
        try:
            (root / "a.py").write_text("abc\n", encoding="utf-8")

            toolkit = ToolkitAdapter()
            toolkit.load_builtin()
            ctx = {"session_root": str(root)}

            grep_res = await toolkit.execute(
                name="grep",
                input={"pattern": "a.c", "glob": "*.py"},
                context=ctx,
            )
            self.assertIsNone(grep_res.error)
            self.assertIn("abc", grep_res.output)

            code_search_res = await toolkit.execute(
                name="code_search",
                input={"query": "a.c", "glob": "*.py"},
                context=ctx,
            )
            self.assertIsNone(code_search_res.error)
            self.assertEqual(code_search_res.output, "")
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_grep_invalid_regex_returns_tool_error(self) -> None:
        root = self._make_temp_root()
        try:
            (root / "a.py").write_text("abc\n", encoding="utf-8")

            toolkit = ToolkitAdapter()
            toolkit.load_builtin()
            res = await toolkit.execute(
                name="grep",
                input={"pattern": "[", "glob": "*.py"},
                context={"session_root": str(root)},
            )

            self.assertIsNotNone(res.error)
            self.assertEqual(res.output, "")
        finally:
            shutil.rmtree(root, ignore_errors=True)
