from __future__ import annotations

import unittest

from openagent.core.mcp.types import RemoteMcpToolDescriptor
from openagent.core.tool.batching import ToolBatchPlanner
from openagent.core.tool.toolkit import ToolkitAdapter
from openagent.core.types import ToolCall


class ToolBatchingTests(unittest.TestCase):
    def test_builtin_tools_have_runtime_execution_schemas(self) -> None:
        toolkit = ToolkitAdapter()
        toolkit.load_builtin()
        tools = {tool.id: tool for tool in toolkit.registry.all()}

        self.assertEqual(tools["read"].execution_schema.concurrency, "safe")
        self.assertTrue(tools["read"].execution_schema.read_only)
        self.assertTrue(tools["read"].execution_schema.mutates_session)
        self.assertEqual(tools["grep"].execution_schema.batch_group, "workspace-read")
        self.assertEqual(tools["write"].execution_schema.concurrency, "exclusive")
        self.assertTrue(tools["write"].execution_schema.mutates_workspace)
        self.assertEqual(tools["edit"].execution_schema.conflict_key_template, "file:{file_path}")
        self.assertEqual(tools["bash"].execution_schema.concurrency, "exclusive")
        self.assertTrue(tools["bash"].execution_schema.mutates_workspace)
        self.assertEqual(tools["memory_read"].execution_schema.concurrency, "safe")
        self.assertEqual(tools["memory_write"].execution_schema.concurrency, "exclusive")
        self.assertEqual(tools["todoread"].execution_schema.concurrency, "safe")
        self.assertEqual(tools["todowrite"].execution_schema.concurrency, "exclusive")
        self.assertTrue(tools["question"].execution_schema.requires_user_interaction)
        self.assertEqual(tools["skill"].execution_schema.batch_group, "skill")
        self.assertEqual(tools["web_fetch"].execution_schema.max_parallelism, 4)
        self.assertEqual(tools["web_search"].execution_schema.max_parallelism, 3)
        self.assertEqual(tools["web_scrape"].execution_schema.max_parallelism, 2)

    def test_planner_groups_consecutive_safe_tools_and_serializes_exclusive_tools(self) -> None:
        toolkit = ToolkitAdapter()
        toolkit.load_builtin()
        calls = [
            ToolCall(name="read", input={"file_path": "README.md"}, call_id="call-read"),
            ToolCall(name="grep", input={"pattern": "OpenAgent"}, call_id="call-grep"),
            ToolCall(name="bash", input={"command": "pwd"}, call_id="call-bash"),
            ToolCall(name="ls", input={"path": "."}, call_id="call-ls"),
        ]

        batches = toolkit.plan_tool_batches(calls)

        self.assertEqual([batch.mode for batch in batches], ["concurrent", "serial", "serial"])
        self.assertEqual([batch.tool_names for batch in batches], [("read", "grep"), ("bash",), ("ls",)])
        self.assertEqual(batches[0].batch_group, "workspace-read")
        self.assertEqual(batches[1].items[0].reason, "exclusive_tool")
        self.assertEqual(batches[2].items[0].reason, "concurrency_safe")

    def test_planner_respects_safe_batch_parallelism_limit(self) -> None:
        toolkit = ToolkitAdapter()
        toolkit.load_builtin()
        calls = [
            ToolCall(name="web_scrape", input={"url": f"https://example.com/{index}"}, call_id=f"call-{index}")
            for index in range(3)
        ]

        batches = ToolBatchPlanner(toolkit.registry).plan(calls)

        self.assertEqual([batch.tool_names for batch in batches], [("web_scrape", "web_scrape"), ("web_scrape",)])
        self.assertEqual([batch.mode for batch in batches], ["concurrent", "serial"])
        self.assertEqual([batch.max_parallelism for batch in batches], [2, 2])

    def test_mcp_tools_default_to_unknown_concurrency(self) -> None:
        class FakeMcpManager:
            def list_tool_descriptors(self) -> list[RemoteMcpToolDescriptor]:
                return [
                    RemoteMcpToolDescriptor(
                        server_name="demo",
                        original_name="echo",
                        dynamic_name="mcp_tool_demo_echo",
                        title="Remote Echo",
                        description="Remote MCP echo tool",
                        input_schema={"type": "object", "properties": {}},
                    )
                ]

            async def call_tool(self, dynamic_name: str, arguments: dict[str, object] | None) -> object:
                raise AssertionError("not executed")

        toolkit = ToolkitAdapter()
        toolkit.register_mcp(FakeMcpManager())
        tool = toolkit.registry.get("mcp_tool_demo_echo")

        self.assertIsNotNone(tool)
        assert tool is not None
        self.assertEqual(tool.execution_schema.concurrency, "unknown")
        self.assertEqual(tool.execution_schema.batch_group, "mcp")
        batches = toolkit.plan_tool_batches([ToolCall(name="mcp_tool_demo_echo", input={}, call_id="call-mcp")])
        self.assertEqual(batches[0].mode, "serial")
        self.assertEqual(batches[0].items[0].reason, "unknown_concurrency")


if __name__ == "__main__":
    unittest.main()
