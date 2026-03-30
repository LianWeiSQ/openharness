from __future__ import annotations

import asyncio
import shutil
import unittest
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

from openagent.core.mcp.types import RemoteMcpToolCallResult, RemoteMcpToolDescriptor
from openagent.core.permission.manager import PermissionDeniedError
from openagent.core.permission.manager import PermissionManager
from openagent.core.permission.ruleset import PermissionRuleset
from openagent.core.question import QuestionManager
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
        root = (tmp_root / f"t_{uuid4().hex}").resolve()
        root.mkdir(parents=True, exist_ok=True)
        return root

    async def test_write_denied_in_readonly(self) -> None:
        pm = PermissionManager()
        pm.set_ruleset(PermissionRuleset.READONLY)
        tk = ToolkitAdapter()
        tk.register_middleware(permission_middleware(pm))
        tk.load_builtin()
        root = self._make_temp_root()
        try:
            with self.assertRaises(PermissionDeniedError):
                await tk.execute(
                    name="write",
                    input={"file_path": "x.txt", "content": "hello"},
                    context={"session_root": str(root)},
                )
        finally:
            shutil.rmtree(root, ignore_errors=True)

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

    async def test_register_mcp_registers_remote_tools_and_executes(self) -> None:
        class FakeMcpManager:
            def __init__(self) -> None:
                self.calls: list[tuple[str, dict[str, object] | None]] = []

            def list_tool_descriptors(self) -> list[RemoteMcpToolDescriptor]:
                return [
                    RemoteMcpToolDescriptor(
                        server_name="demo",
                        original_name="echo",
                        dynamic_name="mcp_tool_demo_echo",
                        title="Remote Echo",
                        description="Remote MCP echo tool",
                        input_schema={
                            "type": "object",
                            "properties": {"value": {"type": "string"}},
                            "required": ["value"],
                        },
                    )
                ]

            async def call_tool(
                self, dynamic_name: str, arguments: dict[str, object] | None
            ) -> RemoteMcpToolCallResult:
                self.calls.append((dynamic_name, arguments))
                return RemoteMcpToolCallResult(
                    output=f"remote:{(arguments or {}).get('value', '')}",
                    metadata={"backend": "mcp", "mcp_server": "demo", "mcp_original_tool_name": "echo", "mcp_transport": "http", "mcp_tool_name": "mcp_tool_demo_echo", "mcp_non_text_blocks": []},
                )

        manager = FakeMcpManager()
        tk = ToolkitAdapter()
        tk.register_mcp(manager)

        tools = {tool.name: tool for tool in tk.get_all_tools()}
        self.assertIn("mcp_tool_demo_echo", tools)
        self.assertTrue(tools["mcp_tool_demo_echo"].dangerous)
        self.assertEqual(tools["mcp_tool_demo_echo"].group, "mcp")

        result = await tk.execute(name="mcp_tool_demo_echo", input={"value": "ok"}, context={})
        self.assertIsNone(result.error)
        self.assertEqual(result.output, "remote:ok")
        self.assertEqual(result.metadata["mcp_original_tool_name"], "echo")
        self.assertEqual(result.metadata["mcp_transport"], "http")
        self.assertEqual(result.metadata["mcp_tool_name"], "mcp_tool_demo_echo")
        self.assertEqual(result.metadata["mcp_non_text_blocks"], [])
        self.assertEqual(manager.calls, [("mcp_tool_demo_echo", {"value": "ok"})])

    async def test_tool_semantic_truncation_is_preserved(self) -> None:
        tk = ToolkitAdapter()
        root = self._make_temp_root()
        try:
            async def run(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
                return ToolOutput(title="semantic", output="partial", metadata={"count": 3}, truncated=True)

            tk.registry.define_tool(tool_id="semantic", parameters=NoArgs, description="# semantic")(run)
            res = await tk.execute(name="semantic", input={}, context={"session_root": str(root)})

            self.assertIsNone(res.error)
            self.assertEqual(res.output, "partial")
            self.assertTrue(res.metadata["truncated"])
            self.assertFalse(res.metadata["output_truncated"])
            self.assertNotIn("output_path", res.metadata)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_output_truncation_writes_full_output(self) -> None:
        tk = ToolkitAdapter()
        root = self._make_temp_root()
        try:
            big_output = "x" * (Truncate.DEFAULT_MAX_BYTES + 32)

            async def run(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
                return ToolOutput(title="big", output=big_output)

            tk.registry.define_tool(tool_id="big", parameters=NoArgs, description="# big")(run)
            res = await tk.execute(name="big", input={}, context={"session_root": str(root)})

            self.assertIsNone(res.error)
            self.assertTrue(res.metadata["truncated"])
            self.assertTrue(res.metadata["output_truncated"])
            output_path = Path(res.metadata["output_path"])
            self.assertTrue(output_path.exists())
            self.assertEqual(output_path.read_text(encoding="utf-8"), big_output)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_display_truncation_uses_context_budget_override(self) -> None:
        tk = ToolkitAdapter()
        root = self._make_temp_root()
        try:
            big_output = "y" * 4096

            async def run(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
                return ToolOutput(title="big", output=big_output)

            tk.registry.define_tool(tool_id="big", parameters=NoArgs, description="# big")(run)
            res = await tk.execute(
                name="big",
                input={},
                context={
                    "session_root": str(root),
                    "agent_options": {"context_budget": {"tool_display_max_bytes": 512}},
                },
            )

            self.assertIsNone(res.error)
            self.assertTrue(res.metadata["output_truncated"])
            self.assertLess(len(res.output.encode("utf-8")), 1024)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_builtin_question_tool_schema_has_questions_only(self) -> None:
        tk = ToolkitAdapter()
        tk.load_builtin()

        tools = {tool.name: tool for tool in tk.get_all_tools()}
        self.assertIn("question", tools)
        schema = tools["question"].schema or {}

        self.assertEqual(set((schema.get("properties") or {}).keys()), {"questions"})
        question_schema = ((schema.get("properties") or {}).get("questions") or {}).get("items") or {}
        question_props = question_schema.get("properties") or {}
        self.assertEqual(set(question_props.keys()), {"header", "question", "options", "multiple"})

    async def test_question_tool_roundtrip_returns_user_answers(self) -> None:
        tk = ToolkitAdapter()
        tk.load_builtin()
        root = self._make_temp_root()
        manager = QuestionManager()
        try:
            task = asyncio.create_task(
                tk.execute(
                    name="question",
                    call_id="question-call-1",
                    input={
                        "questions": [
                            {
                                "header": "Scope",
                                "question": "Which path should we take?",
                                "options": [
                                    {"label": "Fast path", "description": "Ship quickly"},
                                    {"label": "Safe path", "description": "Add extra checks"},
                                ],
                                "multiple": False,
                            }
                        ]
                    },
                    context={
                        "session_id": "session-1",
                        "session_root": str(root),
                        "question_manager": manager,
                    },
                )
            )
            request = await asyncio.wait_for(manager.next_request(), timeout=1)
            self.assertEqual(request.tool_call_id, "question-call-1")
            self.assertEqual(request.questions[0].header, "Scope")

            manager.reply(request.request_id, [["Fast path"]])
            result = await asyncio.wait_for(task, timeout=1)

            self.assertIsNone(result.error)
            self.assertEqual(result.metadata["answers"], [["Fast path"]])
            self.assertEqual(result.metadata["request_id"], request.request_id)
            self.assertIn("Fast path", result.output)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    async def test_semantic_and_output_truncation_do_not_override_each_other(self) -> None:
        tk = ToolkitAdapter()
        root = self._make_temp_root()
        try:
            big_output = "y" * (Truncate.DEFAULT_MAX_BYTES + 64)

            async def run(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
                return ToolOutput(title="both", output=big_output, truncated=True)

            tk.registry.define_tool(tool_id="both", parameters=NoArgs, description="# both")(run)
            res = await tk.execute(name="both", input={}, context={"session_root": str(root)})

            self.assertIsNone(res.error)
            self.assertTrue(res.metadata["truncated"])
            self.assertTrue(res.metadata["output_truncated"])
            self.assertIn("output_path", res.metadata)
        finally:
            shutil.rmtree(root, ignore_errors=True)
