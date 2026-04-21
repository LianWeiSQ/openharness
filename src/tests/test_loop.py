from __future__ import annotations

import shutil
import unittest
from unittest.mock import patch
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.permission.rule import PermissionAction
from openagent.core.session.session import Session
from openagent.core.tool.definition import ToolContext, ToolOutput
from openagent.core.tool.toolkit import ToolkitAdapter
from openagent.core.types import AgentConfig, ChatMessage, Model, SessionStatus

from _mock_model import ScriptedLanguageModel


@dataclass
class NoArgs:
    pass


def _make_model_metadata(*, context_window: int = 256, max_output: int = 64) -> Model:
    return Model(
        id="test-model",
        provider_id="test",
        name="Test Model",
        context_window=context_window,
        max_output=max_output,
    )


def _success_script() -> list[list[dict[str, object]]]:
    return [
        [
            {"type": "text-delta", "id": "t1", "text": "done"},
            {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
        ]
    ]


def _tool_call_step(*, call_id: str, name: str = "ls", input: dict[str, object] | None = None) -> list[dict[str, object]]:
    return [
        {
            "type": "tool-call",
            "call_id": call_id,
            "name": name,
            "input": input or {"path": "."},
        },
        {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
    ]


class LoopTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_dir(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"t_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, td, True)
        return td

    def _runtime_context_messages(self, messages: list[ChatMessage]) -> list[ChatMessage]:
        return [message for message in messages if bool((message.metadata or {}).get("runtime_context"))]

    def _assert_runtime_context_present(self, messages: list[ChatMessage]) -> None:
        runtime_messages = self._runtime_context_messages(messages)
        self.assertEqual(len(runtime_messages), 1)
        runtime = runtime_messages[0]
        self.assertIn("[Runtime context]", runtime.content)
        self.assertRegex(runtime.content, r"Current local datetime: \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}")
        self.assertIn("UTC", runtime.content)

    async def test_loop_executes_tool_and_emits_patch(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "tool-call", "call_id": "c1", "name": "write", "input": {"file_path": "a.txt", "content": "hi"}},
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("hi"):
            events.append(event)

        types = [event["type"] for event in events]
        self.assertIn("tool-result", types)
        self.assertIn("patch", types)

    async def test_sandbox_session_hides_host_only_tools_from_model(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(name="u", permission="FULL", max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        session.metadata["execution"] = {
            "mode": "opensandbox",
            "sandbox_id": "sbx_123",
            "remote_workdir": "/workspace/project",
        }

        fake_runtime = type(
            "FakeRuntime",
            (),
            {
                "mode": "opensandbox",
                "workspace_root": "/workspace/project",
                "execution_metadata": {
                    "execution_mode": "opensandbox",
                    "sandbox_id": "sbx_123",
                    "remote_workdir": "/workspace/project",
                },
            },
        )()

        with patch("openagent.core.loop.processor.build_workspace_runtime", return_value=fake_runtime):
            loop = AgentLoop(agent=agent, session=session, permission_manager=pm)
            events = []
            async for event in loop.run("show available tools"):
                events.append(event)

        self.assertTrue(any(event["type"] == "step-finish" for event in events))
        exposed = set(model.seen_tools_by_call[0])
        self.assertIn("bash", exposed)
        self.assertIn("read", exposed)
        self.assertIn("web_fetch", exposed)
        self.assertNotIn("code_search", exposed)

    async def test_loop_errors_before_provider_call_when_context_budget_overflows(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            model=_make_model_metadata(context_window=96, max_output=24),
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("x" * 800):
            events.append(event)

        self.assertEqual(model.call_index, 0)
        self.assertEqual(events[-1]["type"], "error")
        self.assertIn("Context budget exceeded before model call", events[-1]["error"])
        self.assertIn("estimated_input_tokens=", events[-1]["error"])

    async def test_loop_skips_context_budget_when_disabled(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            model=_make_model_metadata(context_window=96, max_output=24),
            options={"context_budget": {"enabled": False}},
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("x" * 800):
            events.append(event)

        self.assertEqual(model.call_index, 1)
        self.assertIn("text-delta", [event["type"] for event in events])

    async def test_loop_skips_context_budget_without_model_metadata(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(name="u", permission="FULL", max_steps=5, model=None)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("x" * 800):
            events.append(event)

        self.assertEqual(model.call_index, 1)
        self.assertIn("text-delta", [event["type"] for event in events])

    async def test_loop_skips_context_budget_with_non_positive_context_window(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            model=_make_model_metadata(context_window=0, max_output=24),
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("x" * 800):
            events.append(event)

        self.assertEqual(model.call_index, 1)
        self.assertIn("text-delta", [event["type"] for event in events])

    async def test_loop_stops_on_repeated_identical_tool_calls_before_max_steps(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                _tool_call_step(call_id="c1", name="ls", input={"path": "."}),
                _tool_call_step(call_id="c2", name="ls", input={"path": "."}),
                _tool_call_step(call_id="c3", name="ls", input={"path": "."}),
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", max_steps=10)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("列出当前目录文件"):
            events.append(event)

        tool_results = [event for event in events if event["type"] == "tool-result"]
        self.assertEqual(len(tool_results), 2)
        self.assertEqual(events[-1]["type"], "error")
        self.assertIn("Detected repeated tool-call loop", events[-1]["error"])
        self.assertIn('ls {"path": "."}', events[-1]["error"])
        self.assertNotIn("max_steps exceeded", events[-1]["error"])

    async def test_loop_keeps_normal_single_tool_call_flow(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                _tool_call_step(call_id="c1", name="ls", input={"path": "."}),
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("列出当前目录文件"):
            events.append(event)

        self.assertIn("tool-result", [event["type"] for event in events])
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "stop")
        self.assertNotIn("error", [event["type"] for event in events])

    async def test_loop_does_not_flag_different_tool_inputs_as_repeated_loop(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                _tool_call_step(call_id="c1", name="ls", input={"path": "."}),
                _tool_call_step(call_id="c2", name="ls", input={"path": "alpha"}),
                [
                    {"type": "text-delta", "id": "t1", "text": "Reached the final step. Here is the best summary from the completed exploration."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", max_steps=3)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        (session.directory / "alpha").mkdir()
        (session.directory / "beta").mkdir()
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("列出几个目录"):
            events.append(event)

        tool_results = [event for event in events if event["type"] == "tool-result"]
        self.assertEqual(len(tool_results), 2)
        self.assertNotIn("error", [event["type"] for event in events])
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "stop")
        self.assertEqual(model.seen_tools_by_call[-1], [])

    async def test_loop_returns_last_text_only_response_even_when_final_finish_reason_is_length(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                _tool_call_step(call_id="c1", name="ls", input={"path": "."}),
                [
                    {"type": "text-delta", "id": "t1", "text": "Partial final result after tool work."},
                    {"type": "finish", "finish_reason": "length", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", max_steps=2)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("先看目录再总结结果"):
            events.append(event)

        self.assertEqual(model.call_index, 2)
        self.assertEqual(model.seen_tools_by_call[-1], [])
        self.assertNotIn("error", [event["type"] for event in events])
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "length")
        self.assertTrue(any(event["type"] == "text-delta" and "Partial final result" in event["text"] for event in events))
        self.assertEqual(session.messages[-1].role, "assistant")
        self.assertIn("Partial final result", session.messages[-1].content)

    async def test_loop_emits_question_request_and_continues_after_reply(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {
                        "type": "tool-call",
                        "call_id": "q1",
                        "name": "question",
                        "input": {
                            "questions": [
                                {
                                    "header": "Plan",
                                    "question": "Which option should we use?",
                                    "options": [
                                        {"label": "Fast path", "description": "Move quickly"},
                                        {"label": "Safe path", "description": "Be conservative"},
                                    ],
                                    "multiple": False,
                                }
                            ]
                        },
                    },
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "Continuing with the chosen plan."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", tools=["question"], max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("Need a choice"):
            events.append(event)
            if event["type"] == "question-request":
                self.assertEqual(loop.session.status, SessionStatus.PAUSED)
                loop.question_manager.reply(event["request_id"], [["Fast path"]])

        event_types = [event["type"] for event in events]
        self.assertIn("question-request", event_types)
        self.assertIn("tool-result", event_types)
        self.assertLess(event_types.index("question-request"), event_types.index("tool-result"))
        tool_result = next(event for event in events if event["type"] == "tool-result")
        self.assertEqual(tool_result["metadata"]["answers"], [["Fast path"]])
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "stop")
        self.assertEqual(loop.session.status, SessionStatus.STOP)
        self.assertEqual(model.call_index, 2)

    async def test_loop_continues_after_question_reject(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {
                        "type": "tool-call",
                        "call_id": "q1",
                        "name": "question",
                        "input": {
                            "questions": [
                                {
                                    "header": "Plan",
                                    "question": "Which option should we use?",
                                    "options": [
                                        {"label": "Fast path", "description": "Move quickly"},
                                        {"label": "Safe path", "description": "Be conservative"},
                                    ],
                                    "multiple": False,
                                }
                            ]
                        },
                    },
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t2", "text": "The question was dismissed, so here is the safest fallback."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", tools=["question"], max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("Need a choice"):
            events.append(event)
            if event["type"] == "question-request":
                loop.question_manager.reject(event["request_id"])

        tool_result = next(event for event in events if event["type"] == "tool-result")
        self.assertIn("dismissed", tool_result["error"])
        self.assertEqual(tool_result["metadata"]["error_kind"], "question_rejected")
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "stop")
        self.assertEqual(model.call_index, 2)
        self.assertTrue(any(event["type"] == "text-delta" and "safest fallback" in event["text"] for event in events))
        self.assertTrue(
            any(getattr(message, "metadata", {}).get("tool_failure_followup") for message in model.seen_messages_by_call[1])
        )

    async def test_loop_continues_after_permission_denied(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {
                        "type": "tool-call",
                        "call_id": "w1",
                        "name": "write",
                        "input": {"file_path": "blocked.txt", "content": "hi"},
                    },
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t2", "text": "Permission was denied, so I will explain the fallback instead."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="PLAN_ONLY", tools=["write"], max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()

        async def deny(_tool_call: dict[str, object]) -> PermissionAction:
            return PermissionAction.DENY

        pm.ask_user_func = deny
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("Write a file for me"):
            events.append(event)

        tool_result = next(event for event in events if event["type"] == "tool-result")
        self.assertIn("Permission denied", tool_result["error"])
        self.assertEqual(tool_result["metadata"]["error_kind"], "permission_denied")
        self.assertEqual(model.call_index, 2)
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "stop")
        self.assertNotIn("error", [event["type"] for event in events])
        self.assertTrue(any(event["type"] == "text-delta" and "fallback" in event["text"] for event in events))

    async def test_loop_continues_after_tool_exception(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "tool-call", "call_id": "f1", "name": "failing", "input": {}},
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t2", "text": "The tool failed, so here is a manual fallback."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", tools=["failing"], max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        toolkit = ToolkitAdapter()

        async def run_failing(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
            raise RuntimeError("network timeout")

        toolkit.registry.define_tool(tool_id="failing", parameters=NoArgs, description="# failing")(run_failing)
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm, toolkit=toolkit)

        events = []
        async for event in loop.run("Try the failing tool"):
            events.append(event)

        tool_result = next(event for event in events if event["type"] == "tool-result")
        self.assertIn("network timeout", tool_result["error"])
        self.assertEqual(tool_result["metadata"]["error_kind"], "tool_error")
        self.assertEqual(model.call_index, 2)
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "stop")
        self.assertNotIn("error", [event["type"] for event in events])
        self.assertTrue(any(event["type"] == "text-delta" and "manual fallback" in event["text"] for event in events))

    async def test_loop_projects_tool_output_before_storing_in_session(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                _tool_call_step(call_id="big1", name="big"),
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", max_steps=5, model=_make_model_metadata(context_window=512, max_output=64), tools=["big"])
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        toolkit = ToolkitAdapter()

        async def run_big(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
            return ToolOutput(title="Big search", output=("alpha\n" * 4000), metadata={"count": 4000})

        toolkit.registry.define_tool(tool_id="big", parameters=NoArgs, description="# big")(run_big)
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm, toolkit=toolkit)

        events = []
        async for event in loop.run("run big tool"):
            events.append(event)

        tool_messages = [message for message in session.messages if message.role == "tool"]
        self.assertEqual(len(tool_messages), 1)
        self.assertIn("[Tool result] big", tool_messages[0].content)
        self.assertIn("preview:", tool_messages[0].content)
        self.assertIn("full_output=", tool_messages[0].content)
        self.assertLess(len(tool_messages[0].content.encode("utf-8")), 5000)
        self.assertIn("output_path", [key for key in tool_messages[0].metadata.keys()])

    async def test_loop_prunes_old_tool_output_before_model_call(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            tools=[],
            model=_make_model_metadata(context_window=2048, max_output=64),
            options={
                "context_budget": {
                    "bytes_per_token": 1,
                    "prune_protect_input_tokens": 0,
                    "prune_min_input_tokens": 1,
                }
            },
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        session.add(ChatMessage(role="user", content="old question"))
        session.add(
            ChatMessage(
                role="tool",
                name="grep",
                tool_call_id="old-1",
                content="x" * 8000,
                metadata={"title": "Old grep", "count": 100, "truncated": True, "output_path": "old.txt"},
            )
        )
        session.add(ChatMessage(role="assistant", content="working"))
        session.add(ChatMessage(role="user", content="recent question"))
        session.add(
            ChatMessage(
                role="tool",
                name="grep",
                tool_call_id="recent-1",
                content="y" * 200,
                metadata={"title": "Recent grep", "count": 1, "truncated": False},
            )
        )
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        async for _event in loop.run("current question"):
            pass

        tool_messages = [message for message in session.messages if message.role == "tool"]
        self.assertIn("[Old tool result content cleared]", tool_messages[0].content)
        self.assertTrue(tool_messages[0].metadata["compacted"])
        self.assertEqual(tool_messages[1].content, "y" * 200)

    async def test_loop_compact_strategy_summarizes_and_continues(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "summary", "text": "Goal: continue implementing"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            tools=[],
            model=_make_model_metadata(context_window=220, max_output=24),
            options={"context_budget": {"strategy": "compact", "reserve_output_tokens": 24}},
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        session.add(ChatMessage(role="user", content="A" * 240))
        session.add(ChatMessage(role="assistant", content="B" * 240))
        session.add(ChatMessage(role="user", content="keep this turn"))
        session.add(ChatMessage(role="assistant", content="recent answer"))
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("current ask"):
            events.append(event)

        self.assertEqual(model.call_index, 2)
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "stop")
        self.assertIn("context_compaction", session.metadata)
        self.assertEqual(session.metadata["context_compaction"]["summary"], "Goal: continue implementing")

    async def test_loop_compact_strategy_falls_back_to_error_when_summary_fails(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 0, "cost": 0.0}},
                ]
            ]
        )
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            tools=[],
            model=_make_model_metadata(context_window=220, max_output=24),
            options={"context_budget": {"strategy": "compact", "reserve_output_tokens": 24}},
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        session.add(ChatMessage(role="user", content="A" * 240))
        session.add(ChatMessage(role="assistant", content="B" * 240))
        session.add(ChatMessage(role="user", content="keep this turn"))
        session.add(ChatMessage(role="assistant", content="recent answer"))
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("current ask"):
            events.append(event)

        self.assertEqual(model.call_index, 1)
        self.assertEqual(events[-1]["type"], "error")
        self.assertIn("Context budget exceeded before model call", events[-1]["error"])

    async def test_loop_handles_two_large_search_tools_without_budget_error(self) -> None:
        root = self._make_temp_dir()
        long_line = "beta " + ("x" * 200)
        (root / "big.py").write_text("\n".join(long_line for _ in range(220)), encoding="utf-8")

        model = ScriptedLanguageModel(
            script=[
                _tool_call_step(call_id="code-1", name="code_search", input={"query": "beta", "glob": "*.py"}),
                _tool_call_step(call_id="grep-1", name="grep", input={"pattern": "beta", "glob": "*.py"}),
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            tools=["code_search", "grep"],
            model=_make_model_metadata(context_window=5000, max_output=64),
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=root)
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("search twice then continue"):
            events.append(event)

        self.assertEqual(model.call_index, 3)
        self.assertNotIn("error", [event["type"] for event in events])
        self.assertEqual(events[-1]["type"], "step-finish")
        tool_messages = [message for message in session.messages if message.role == "tool"]
        self.assertEqual(len(tool_messages), 2)
        self.assertTrue(all(message.content.startswith("[Tool result]") for message in tool_messages))







    async def test_loop_reports_explicit_error_when_current_user_input_alone_exceeds_context(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            model=_make_model_metadata(context_window=96, max_output=24),
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        events = []
        async for event in loop.run("x" * 800):
            events.append(event)

        self.assertEqual(model.call_index, 0)
        self.assertEqual(events[-1]["type"], "error")
        self.assertIn("Current user input is too large", events[-1]["error"])

    async def test_loop_auto_strategy_uses_text_only_final_attempt_after_trim(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "final", "text": "Recovered answer from trimmed context."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 12, "output_tokens": 6, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            tools=["huge"],
            model=_make_model_metadata(context_window=900, max_output=64),
            options={
                "context_budget": {
                    "bytes_per_token": 1,
                    "overflow_keep_recent_user_turns": 1,
                    "overflow_final_max_output_tokens": 32,
                }
            },
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        session.add(ChatMessage(role="user", content="old request"))
        session.add(ChatMessage(role="assistant", content="A" * 400))
        toolkit = ToolkitAdapter()

        async def run_huge(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
            return ToolOutput(title="Huge", output="unused")

        toolkit.registry.define_tool(tool_id="huge", parameters=NoArgs, description=("A" * 1200))(run_huge)
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm, toolkit=toolkit)

        events = []
        async for event in loop.run("current ask"):
            events.append(event)

        self.assertEqual(model.call_index, 1)
        self.assertEqual(model.seen_tools_by_call[-1], [])
        self.assertEqual(model.seen_max_output_tokens_by_call[-1], 32)
        self.assertTrue(any("CONTEXT OVERFLOW RECOVERY" in message.content for message in model.seen_messages_by_call[-1]))
        self.assertEqual(session.metadata["last_context_budget"]["reserved_output_tokens"], 32)
        self.assertNotIn("error", [event["type"] for event in events])
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "stop")

    async def test_loop_final_text_only_error_reports_overflow_final_reserved_output_tokens(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                _tool_call_step(call_id="big-1", name="big"),
            ]
        )
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            tools=["big"],
            model=_make_model_metadata(context_window=520, max_output=64),
            options={
                "context_budget": {
                    "bytes_per_token": 1,
                    "overflow_keep_recent_user_turns": 1,
                    "overflow_final_max_output_tokens": 32,
                }
            },
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        toolkit = ToolkitAdapter()

        async def run_big(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
            return ToolOutput(title="Big search", output="alpha\n" * 2000, metadata={"count": 2000})

        toolkit.registry.define_tool(tool_id="big", parameters=NoArgs, description=("A" * 120))(run_big)
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm, toolkit=toolkit)

        events = []
        async for event in loop.run("current ask"):
            events.append(event)

        self.assertEqual(model.call_index, 1)
        self.assertEqual(events[-1]["type"], "error")
        self.assertIn("fallback_stage=final_text_only", events[-1]["error"])
        self.assertIn("reserved_output_tokens=32", events[-1]["error"])

    async def test_loop_records_last_model_usage_metadata(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(name="u", permission="FULL", max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        async for _event in loop.run("hello"):
            pass

        self.assertEqual(session.metadata["last_model_usage"]["input_tokens"], 1)
        self.assertEqual(session.metadata["last_model_usage"]["output_tokens"], 1)
        self.assertIn("last_model_usage_at", session.metadata)

    async def test_loop_injects_runtime_context_message_without_persisting_it(self) -> None:
        model = ScriptedLanguageModel(script=_success_script())
        cfg = AgentConfig(name="u", permission="FULL", max_steps=5)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        async for _event in loop.run("hello"):
            pass

        self.assertEqual(model.call_index, 1)
        self._assert_runtime_context_present(model.seen_messages_by_call[0])
        self.assertFalse(any(bool((message.metadata or {}).get("runtime_context")) for message in session.messages))

    async def test_loop_final_step_messages_include_runtime_context(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                _tool_call_step(call_id="c1", name="ls", input={"path": "."}),
                [
                    {"type": "text-delta", "id": "t1", "text": "Final text-only answer."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(name="u", permission="FULL", max_steps=2)
        agent = UniversalAgent(config=cfg, model=model, system_prompt="Test prompt.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        async for _event in loop.run("list then summarize"):
            pass

        final_messages = model.seen_messages_by_call[-1]
        self._assert_runtime_context_present(final_messages)
        self.assertTrue(any("CRITICAL - MAXIMUM STEPS REACHED" in message.content for message in final_messages))

    async def test_loop_compaction_summary_messages_include_runtime_context(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "summary", "text": "Goal: continue implementing"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            tools=[],
            model=_make_model_metadata(context_window=220, max_output=24),
            options={"context_budget": {"strategy": "compact", "reserve_output_tokens": 24}},
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        session.add(ChatMessage(role="user", content="A" * 240))
        session.add(ChatMessage(role="assistant", content="B" * 240))
        session.add(ChatMessage(role="user", content="keep this turn"))
        session.add(ChatMessage(role="assistant", content="recent answer"))
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm)

        async for _event in loop.run("current ask"):
            pass

        self._assert_runtime_context_present(model.seen_messages_by_call[0])

    async def test_loop_overflow_final_attempt_messages_include_runtime_context(self) -> None:
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "final", "text": "Recovered answer from trimmed context."},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 12, "output_tokens": 6, "cost": 0.0}},
                ],
            ]
        )
        cfg = AgentConfig(
            name="u",
            permission="FULL",
            max_steps=5,
            tools=["huge"],
            model=_make_model_metadata(context_window=900, max_output=64),
            options={
                "context_budget": {
                    "bytes_per_token": 1,
                    "overflow_keep_recent_user_turns": 1,
                    "overflow_final_max_output_tokens": 32,
                }
            },
        )
        agent = UniversalAgent(config=cfg, model=model, system_prompt="You are helpful.")
        pm = PermissionManager()
        session = Session(directory=self._make_temp_dir())
        session.add(ChatMessage(role="user", content="old request"))
        session.add(ChatMessage(role="assistant", content="A" * 400))
        toolkit = ToolkitAdapter()

        async def run_huge(_args: NoArgs, _ctx: ToolContext) -> ToolOutput:
            return ToolOutput(title="Huge", output="unused")

        toolkit.registry.define_tool(tool_id="huge", parameters=NoArgs, description=("A" * 1200))(run_huge)
        loop = AgentLoop(agent=agent, session=session, permission_manager=pm, toolkit=toolkit)

        async for _event in loop.run("current ask"):
            pass

        final_messages = model.seen_messages_by_call[-1]
        self._assert_runtime_context_present(final_messages)
        self.assertTrue(any("CONTEXT OVERFLOW RECOVERY" in message.content for message in final_messages))
