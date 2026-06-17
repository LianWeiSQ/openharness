from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.app_server.runtime import OpenAgentAppRuntime

from _mock_model import ScriptedLanguageModel


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


if __name__ == "__main__":
    unittest.main()
