from __future__ import annotations

import asyncio
import shutil
import threading
import time
import unittest
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any
from uuid import uuid4
from unittest.mock import patch

from openagent.core.provider.base import LanguageModel
from openagent.app_server.runtime import OpenAgentAppRuntime

from _mock_model import ScriptedLanguageModel


class InterruptibleLanguageModel(LanguageModel):
    def __init__(self) -> None:
        self.first_chunk_yielded = threading.Event()
        self.release = threading.Event()

    async def stream(
        self,
        *,
        system: str | None,
        messages: list[Any],
        tools: list[Any],
        temperature: float | None = None,
        max_output_tokens: int | None = None,
        options: dict[str, Any] | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        del system, messages, tools, temperature, max_output_tokens, options
        yield {"type": "text-delta", "id": "t1", "text": "before interrupt"}
        self.first_chunk_yielded.set()
        for _ in range(500):
            if self.release.is_set():
                break
            await asyncio.sleep(0.01)
        yield {"type": "text-delta", "id": "t2", "text": "after interrupt"}
        yield {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}}


class AppServerRuntimeTests(unittest.TestCase):
    def _make_temp_dir(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"app_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, td, True)
        return td

    def test_runtime_runs_turn_and_records_ui_events(self) -> None:
        workspace = self._make_temp_dir()
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "text-delta", "id": "t1", "text": "hello from app bridge"},
                    {
                        "type": "finish",
                        "finish_reason": "stop",
                        "usage": {"input_tokens": 2, "output_tokens": 3, "cost": 0.0},
                    },
                ]
            ]
        )
        runtime = OpenAgentAppRuntime(
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            language_model_factory=lambda _model: model,
        )

        session = runtime.start_session()
        turn = runtime.start_turn(session_id=session["id"], user_text="say hello")

        self.assertTrue(turn.wait_until_terminal(timeout_s=10.0))
        methods = [event.method for event in turn.events]

        self.assertEqual(turn.status, "completed")
        self.assertEqual(turn.final_answer, "hello from app bridge")
        self.assertIn("turn/started", methods)
        self.assertIn("item/agentMessage/delta", methods)
        self.assertIn("item/step/completed", methods)
        self.assertIn("turn/completed", methods)

        restored = runtime.get_session(session["id"])
        self.assertGreaterEqual(restored["message_count"], 2)

    def test_runtime_interrupts_running_turn_at_event_boundary(self) -> None:
        workspace = self._make_temp_dir()
        model = InterruptibleLanguageModel()
        runtime = OpenAgentAppRuntime(
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            language_model_factory=lambda _model: model,
        )

        session = runtime.start_session()
        turn = runtime.start_turn(session_id=session["id"], user_text="stream slowly")

        self.assertTrue(model.first_chunk_yielded.wait(timeout=10.0))
        payload = runtime.interrupt_turn(turn.id)
        self.assertEqual(payload["status"], "interrupting")
        model.release.set()

        self.assertTrue(turn.wait_until_terminal(timeout_s=10.0))
        methods = [event.method for event in turn.events]
        timeline_text = "\n".join(str(event.params) for event in turn.events)

        self.assertEqual(turn.status, "interrupted")
        self.assertTrue(turn.interrupt_requested)
        self.assertIn("turn/interrupt_requested", methods)
        self.assertIn("turn/interrupted", methods)
        self.assertNotIn("turn/completed", methods)
        self.assertIn("before interrupt", turn.final_answer)
        self.assertNotIn("after interrupt", timeline_text)

    def test_runtime_pauses_for_tool_approval_and_continues_after_deny(self) -> None:
        workspace = self._make_temp_dir()
        model = ScriptedLanguageModel(
            script=[
                [
                    {
                        "type": "tool-call",
                        "call_id": "w1",
                        "name": "write",
                        "input": {"file_path": "blocked.txt", "content": "hi"},
                    },
                    {
                        "type": "finish",
                        "finish_reason": "tool_call",
                        "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0},
                    },
                ],
                [
                    {"type": "text-delta", "id": "t2", "text": "Denied fallback."},
                    {
                        "type": "finish",
                        "finish_reason": "stop",
                        "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0},
                    },
                ],
            ]
        )
        runtime = OpenAgentAppRuntime(
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            language_model_factory=lambda _model: model,
        )

        with patch.dict("os.environ", {"OPENAGENT_APP_PERMISSION": "PLAN_ONLY", "OPENAGENT_APP_TOOLS": "write"}):
            session = runtime.start_session()
            turn = runtime.start_turn(session_id=session["id"], user_text="write a file")

            approval_event = self._wait_for_method(turn, "turn/approval_requested")
            approval = approval_event.params["approval"]
            self.assertEqual(turn.status, "waiting_approval")
            self.assertEqual(approval["tool_name"], "write")

            resolved = runtime.respond_approval(turn.id, approval["request_id"], "deny")

            self.assertEqual(resolved["method"], "turn/approval_resolved")
            self.assertEqual(resolved["params"]["approval"]["action"], "deny")
            self.assertTrue(turn.wait_until_terminal(timeout_s=10.0))

        methods = [event.method for event in turn.events]
        tool_results = [event for event in turn.events if event.method == "item/toolCall/completed"]
        self.assertEqual(turn.status, "completed")
        self.assertIn("turn/approval_requested", methods)
        self.assertIn("turn/approval_resolved", methods)
        self.assertEqual(tool_results[0].params["event"]["metadata"]["error_kind"], "permission_denied")
        self.assertEqual(turn.final_answer, "Denied fallback.")
        self.assertFalse((workspace / "blocked.txt").exists())

    def test_runtime_interrupt_resolves_pending_approval(self) -> None:
        workspace = self._make_temp_dir()
        model = ScriptedLanguageModel(
            script=[
                [
                    {
                        "type": "tool-call",
                        "call_id": "w1",
                        "name": "write",
                        "input": {"file_path": "blocked.txt", "content": "hi"},
                    },
                    {
                        "type": "finish",
                        "finish_reason": "tool_call",
                        "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0},
                    },
                ],
            ]
        )
        runtime = OpenAgentAppRuntime(
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            language_model_factory=lambda _model: model,
        )

        with patch.dict("os.environ", {"OPENAGENT_APP_PERMISSION": "PLAN_ONLY", "OPENAGENT_APP_TOOLS": "write"}):
            session = runtime.start_session()
            turn = runtime.start_turn(session_id=session["id"], user_text="write a file")

            approval_event = self._wait_for_method(turn, "turn/approval_requested")
            self.assertEqual(approval_event.params["approval"]["tool_name"], "write")

            runtime.interrupt_turn(turn.id)
            self.assertTrue(turn.wait_until_terminal(timeout_s=10.0))

        methods = [event.method for event in turn.events]
        resolved = [event for event in turn.events if event.method == "turn/approval_resolved"]
        self.assertEqual(turn.status, "interrupted")
        self.assertIn("turn/interrupt_requested", methods)
        self.assertEqual(resolved[0].params["approval"]["action"], "deny")
        self.assertEqual(resolved[0].params["approval"]["reason"], "interrupt")

    def _wait_for_method(self, turn, method: str):
        deadline = time.time() + 10.0
        sequence = 1
        while time.time() < deadline:
            event = turn.wait_for_sequence(sequence, timeout_s=max(0.05, deadline - time.time()))
            if event is None:
                break
            if event.method == method:
                return event
            sequence += 1
        self.fail(f"timed out waiting for {method}")


if __name__ == "__main__":
    unittest.main()
