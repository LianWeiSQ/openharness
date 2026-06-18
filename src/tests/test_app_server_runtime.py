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
from openagent.core.provider.anthropic import AnthropicProvider
from openagent.core.types import Model
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


class OptionCapturingLanguageModel(LanguageModel):
    def __init__(self) -> None:
        self.options_by_call: list[dict[str, Any] | None] = []

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
        del system, messages, tools, temperature, max_output_tokens
        self.options_by_call.append(dict(options) if isinstance(options, dict) else None)
        yield {"type": "text-delta", "id": "t1", "text": "selected runtime"}
        yield {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0.0}}


class FakeProvider:
    async def list_models(self) -> list[Model]:
        return [
            Model(id="model-a", provider_id="test", name="Model A", context_window=1024, max_output=128),
            Model(id="model-b", provider_id="test", name="Model B", context_window=2048, max_output=256),
        ]

    async def get_language_model(self, model: Model) -> LanguageModel:
        del model
        return ScriptedLanguageModel(script=[])


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

    def test_runtime_selects_anthropic_provider_from_environment(self) -> None:
        workspace = self._make_temp_dir()
        with patch.dict(
            "os.environ",
            {
                "OPENAGENT_PROVIDER": "anthropic",
                "ANTHROPIC_API_KEY": "test",
                "ANTHROPIC_MODEL": "claude-runtime",
            },
            clear=True,
        ):
            runtime = OpenAgentAppRuntime(
                workspace=workspace,
                session_store_root=workspace / ".openagent" / "sessions",
                language_model_factory=lambda _model: ScriptedLanguageModel(script=[]),
            )
            models = runtime.list_models()

        self.assertIsInstance(runtime.provider, AnthropicProvider)
        self.assertEqual(models[0]["provider_id"], "anthropic")
        self.assertEqual(models[0]["id"], "claude-runtime")

    def test_runtime_applies_selected_model_agent_and_variant(self) -> None:
        workspace = self._make_temp_dir()
        selected_model_ids: list[str] = []
        model = OptionCapturingLanguageModel()
        runtime = OpenAgentAppRuntime(
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            language_model_factory=lambda selected: selected_model_ids.append(selected.id) or model,
        )
        runtime.provider = FakeProvider()  # type: ignore[assignment]

        session = runtime.start_session()
        turn = runtime.start_turn(session_id=session["id"], user_text="use selected runtime", model_id="model-b", agent="plan", variant="high")

        self.assertTrue(turn.wait_until_terminal(timeout_s=10.0))
        self.assertEqual(turn.status, "completed")
        self.assertEqual(turn.model_id, "model-b")
        self.assertEqual(turn.agent_name, "plan")
        self.assertEqual(turn.variant, "high")
        self.assertEqual(selected_model_ids, ["model-b"])
        self.assertEqual(model.options_by_call[0]["selected_agent"], "plan")
        self.assertEqual(model.options_by_call[0]["selected_model_id"], "model-b")
        self.assertEqual(model.options_by_call[0]["model_variant"], "high")
        self.assertEqual(model.options_by_call[0]["reasoning_effort"], "high")

    def test_runtime_marks_turn_failed_for_unknown_selected_model(self) -> None:
        workspace = self._make_temp_dir()
        runtime = OpenAgentAppRuntime(
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            language_model_factory=lambda _model: ScriptedLanguageModel(script=[]),
        )
        runtime.provider = FakeProvider()  # type: ignore[assignment]

        session = runtime.start_session()
        turn = runtime.start_turn(session_id=session["id"], user_text="use missing model", model_id="missing-model")

        self.assertTrue(turn.wait_until_terminal(timeout_s=10.0))
        self.assertEqual(turn.status, "failed")
        self.assertIn("unknown model id: missing-model", turn.error or "")

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

    def test_runtime_tui_control_queue_and_response_store(self) -> None:
        workspace = self._make_temp_dir()
        runtime = OpenAgentAppRuntime(
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            language_model_factory=lambda _model: ScriptedLanguageModel(script=[]),
        )

        request = runtime.enqueue_tui_control("/tui/append-prompt", {"text": "hello"})
        received = runtime.wait_for_tui_control(timeout_s=0.0)
        empty = runtime.wait_for_tui_control(timeout_s=0.0)
        response = runtime.record_tui_control_response({"ok": True, "result": {"applied": True}})

        self.assertIsNotNone(received)
        assert received is not None
        self.assertEqual(request.to_dict(), {"path": "/tui/append-prompt", "body": {"text": "hello"}})
        self.assertEqual(received.to_dict(), {"path": "/tui/append-prompt", "body": {"text": "hello"}})
        self.assertIsNone(empty)
        self.assertEqual(response, {"ok": True, "result": {"applied": True}})
        self.assertEqual(runtime.next_tui_control_response(), response)

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
