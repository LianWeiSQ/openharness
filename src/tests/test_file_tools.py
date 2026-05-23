from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.file_context import FILE_CONTEXT_METADATA_KEY, FileContextState
from openagent.core.session.session import Session
from openagent.core.tool.toolkit import ToolkitAdapter


class FileToolTests(unittest.IsolatedAsyncioTestCase):
    async def test_basic_file_tools(self) -> None:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        root = (tmp_root / f"t_{uuid4().hex}").resolve()
        root.mkdir(parents=True, exist_ok=True)
        try:
            toolkit = ToolkitAdapter()
            toolkit.load_builtin()
            ctx = {"session_root": str(root)}

            res = await toolkit.execute(
                name="write",
                input={"file_path": "a.txt", "content": "hello\nworld"},
                context=ctx,
            )
            self.assertIsNone(res.error)
            self.assertTrue((root / "a.txt").exists())

            res = await toolkit.execute(
                name="read",
                input={"file_path": "a.txt", "offset": 0, "limit": 2},
                context=ctx,
            )
            self.assertIsNone(res.error)
            self.assertIn("00001| hello", res.output)
            self.assertIn("00002| world", res.output)

            res = await toolkit.execute(
                name="edit",
                input={"file_path": "a.txt", "old_string": "world", "new_string": "there"},
                context=ctx,
            )
            self.assertIsNone(res.error)
            self.assertIn("there", (root / "a.txt").read_text(encoding="utf-8"))

            (root / "dir").mkdir()
            res = await toolkit.execute(name="ls", input={}, context=ctx)
            self.assertIsNone(res.error)
            self.assertIn("a.txt", res.output)
            self.assertIn("dir/", res.output)

            (root / "b.md").write_text("x", encoding="utf-8")
            res = await toolkit.execute(name="glob", input={"pattern": "*.txt"}, context=ctx)
            self.assertIsNone(res.error)
            self.assertIn(str(root / "a.txt"), res.output)

            res = await toolkit.execute(name="grep", input={"pattern": "hello"}, context=ctx)
            self.assertIsNone(res.error)
            self.assertIn("hello", res.output)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_existing_file_requires_read_before_write_or_edit(self) -> None:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        root = (tmp_root / f"t_{uuid4().hex}").resolve()
        root.mkdir(parents=True, exist_ok=True)
        try:
            toolkit = ToolkitAdapter()
            toolkit.load_builtin()
            session = Session(directory=root)
            ctx = {"session_root": str(root), "session": session}
            (root / "a.txt").write_text("hello hello", encoding="utf-8")

            write_res = await toolkit.execute(
                name="write",
                input={"file_path": "a.txt", "content": "updated"},
                context=ctx,
            )
            self.assertIsNotNone(write_res.error)
            self.assertIn("Must read existing file before writing it", write_res.error or "")

            edit_res = await toolkit.execute(
                name="edit",
                input={"file_path": "a.txt", "old_string": "hello", "new_string": "hi"},
                context=ctx,
            )
            self.assertIsNotNone(edit_res.error)
            self.assertIn("Must read existing file before editing it", edit_res.error or "")

            read_res = await toolkit.execute(name="read", input={"file_path": "a.txt"}, context=ctx)
            self.assertIsNone(read_res.error)

            ambiguous_edit = await toolkit.execute(
                name="edit",
                input={"file_path": "a.txt", "old_string": "hello", "new_string": "hi"},
                context=ctx,
            )
            self.assertIsNotNone(ambiguous_edit.error)
            self.assertIn("old_string found multiple times", ambiguous_edit.error or "")

            replace_all_res = await toolkit.execute(
                name="edit",
                input={"file_path": "a.txt", "old_string": "hello", "new_string": "hi", "replace_all": True},
                context=ctx,
            )
            self.assertIsNone(replace_all_res.error)
            self.assertEqual((root / "a.txt").read_text(encoding="utf-8"), "hi hi")
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_file_tools_update_file_context_state(self) -> None:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        root = (tmp_root / f"t_{uuid4().hex}").resolve()
        root.mkdir(parents=True, exist_ok=True)
        try:
            toolkit = ToolkitAdapter()
            toolkit.load_builtin()
            session = Session(directory=root)
            ctx = {"session_root": str(root), "session": session}

            write_res = await toolkit.execute(
                name="write",
                input={"file_path": "tracked.txt", "content": "alpha"},
                context=ctx,
            )
            self.assertIsNone(write_res.error)
            self.assertIn(FILE_CONTEXT_METADATA_KEY, session.metadata)
            state = FileContextState.from_metadata(session.metadata)
            record = state.records[str((root / "tracked.txt").resolve())]
            self.assertEqual(record.path, "tracked.txt")
            self.assertEqual(record.source_tool, "write")
            self.assertIn("alpha", record.preview)

            read_res = await toolkit.execute(name="read", input={"file_path": "tracked.txt"}, context=ctx)
            self.assertIsNone(read_res.error)
            state = FileContextState.from_metadata(session.metadata)
            record = state.records[str((root / "tracked.txt").resolve())]
            self.assertEqual(record.source_tool, "read")

            edit_res = await toolkit.execute(
                name="edit",
                input={"file_path": "tracked.txt", "old_string": "alpha", "new_string": "beta"},
                context=ctx,
            )
            self.assertIsNone(edit_res.error)
            state = FileContextState.from_metadata(session.metadata)
            record = state.records[str((root / "tracked.txt").resolve())]
            self.assertEqual(record.source_tool, "edit")
            self.assertIn("beta", record.preview)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_path_escape_is_blocked(self) -> None:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        root = (tmp_root / f"t_{uuid4().hex}").resolve()
        root.mkdir(parents=True, exist_ok=True)
        try:
            toolkit = ToolkitAdapter()
            toolkit.load_builtin()
            ctx = {"session_root": str(root)}

            outside = root.parent / "outside.txt"
            res = await toolkit.execute(
                name="read",
                input={"file_path": str(outside)},
                context=ctx,
            )
            self.assertIsNotNone(res.error)
            self.assertIn("Path escapes session root", res.error or "")
        finally:
            shutil.rmtree(root, ignore_errors=True)
