from __future__ import annotations

import json
import unittest

from openagent.adapter.memory_adapter import MemoryAdapter
from openagent.core.tool.toolkit import ToolkitAdapter


class MemoryToolTests(unittest.IsolatedAsyncioTestCase):
    async def test_memory_read_write(self) -> None:
        toolkit = ToolkitAdapter()
        toolkit.load_builtin()

        mem = MemoryAdapter()
        ctx = {"memory": mem}

        res = await toolkit.execute(
            name="memory_write",
            input={"key": "k1", "value": {"x": 1}},
            context=ctx,
        )
        self.assertIsNone(res.error)
        self.assertEqual(res.output, "ok")

        res = await toolkit.execute(
            name="memory_read",
            input={"key": "k1"},
            context=ctx,
        )
        self.assertIsNone(res.error)
        self.assertEqual(json.loads(res.output), {"x": 1})
