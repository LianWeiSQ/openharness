from __future__ import annotations

import json
import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.session.session import Session
from openagent.core.tool.toolkit import ToolkitAdapter


class TodoToolTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_root(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        root = (tmp_root / f"t_{uuid4().hex}").resolve()
        root.mkdir(parents=True, exist_ok=True)
        return root

    async def test_todowrite_and_todo_roundtrip(self) -> None:
        root = self._make_temp_root()
        try:
            toolkit = ToolkitAdapter()
            toolkit.load_builtin()
            session = Session(directory=root)
            ctx = {"session_root": str(root), "session_id": session.id, "session": session}

            payload = {
                "todos": [
                    {"id": "inspect", "content": "Inspect tool chain", "status": "in_progress", "priority": "high"},
                    {"id": "docs", "content": "Update tool docs", "status": "pending", "priority": "medium"},
                ]
            }
            write_res = await toolkit.execute(name="todowrite", input=payload, context=ctx)
            self.assertIsNone(write_res.error)
            self.assertEqual(len(session.todos), 2)
            self.assertEqual(write_res.metadata["todos"][0]["id"], "inspect")

            read_res = await toolkit.execute(name="todo", input={}, context=ctx)
            self.assertIsNone(read_res.error)
            data = json.loads(read_res.output)
            self.assertEqual(data[0]["content"], "Inspect tool chain")
            self.assertEqual(data[1]["status"], "pending")
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_todowrite_schema_contains_nested_object_fields(self) -> None:
        toolkit = ToolkitAdapter()
        toolkit.load_builtin()
        tools = {tool.name: tool for tool in toolkit.get_all_tools()}
        schema = tools["todowrite"].schema
        self.assertIsNotNone(schema)
        todos_schema = schema["properties"]["todos"]
        self.assertEqual(todos_schema["type"], "array")
        self.assertEqual(todos_schema["items"]["type"], "object")
        self.assertIn("content", todos_schema["items"]["properties"])
        self.assertIn("status", todos_schema["items"]["properties"])
