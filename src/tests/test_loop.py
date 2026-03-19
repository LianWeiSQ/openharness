from __future__ import annotations

import shutil
import unittest
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
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

    async def test_loop_stops_after_question_reject(self) -> None:
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
                ]
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
        self.assertEqual(events[-1]["type"], "step-finish")
        self.assertEqual(events[-1]["finish_reason"], "tool_call")
        self.assertEqual(model.call_index, 1)

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






