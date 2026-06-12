from __future__ import annotations

import json
import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.agent.universal import UniversalAgent
from openagent.core.loop.processor import AgentLoop
from openagent.core.permission.manager import PermissionManager
from openagent.core.session import FileSessionStore, SESSION_STORE_METADATA_KEY, Session
from openagent.core.types import AgentConfig

from _mock_model import ScriptedLanguageModel


class SessionStoreTests(unittest.IsolatedAsyncioTestCase):
    def _make_temp_dir(self) -> Path:
        root = Path("src/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = root / f"session_store_{uuid4().hex}"
        path.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    async def test_file_session_store_records_three_step_run_and_restores_messages(self) -> None:
        temp = self._make_temp_dir()
        model = ScriptedLanguageModel(
            script=[
                [
                    {
                        "type": "tool-call",
                        "call_id": "write-1",
                        "name": "write",
                        "input": {"file_path": "answer.txt", "content": "hello ledger"},
                    },
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 2, "output_tokens": 3, "cost": 0.01}},
                ],
                [
                    {
                        "type": "tool-call",
                        "call_id": "read-1",
                        "name": "read",
                        "input": {"file_path": "answer.txt", "offset": 0, "limit": 10},
                    },
                    {"type": "finish", "finish_reason": "tool_call", "usage": {"input_tokens": 4, "output_tokens": 5, "cost": 0.02}},
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "done"},
                    {"type": "finish", "finish_reason": "stop", "usage": {"input_tokens": 6, "output_tokens": 7, "cost": 0.03}},
                ],
            ]
        )
        session = Session(directory=temp)
        agent = UniversalAgent(
            config=AgentConfig(
                name="session-store-test",
                permission="FULL",
                tools=["write", "read"],
                max_steps=5,
                options={"session_store": {"root_dir": ".openagent/sessions"}},
            ),
            model=model,
            system_prompt="Test agent.",
        )
        loop = AgentLoop(agent=agent, session=session, permission_manager=PermissionManager())

        events = [event async for event in loop.run("write and read answer.txt")]

        self.assertEqual(model.call_index, 3)
        self.assertTrue((temp / "answer.txt").exists())
        self.assertTrue(any(event["type"] == "patch" for event in events))
        metadata = session.metadata[SESSION_STORE_METADATA_KEY]
        ledger_path = Path(metadata["ledger_path"])
        state_path = Path(metadata["state_path"])
        self.assertTrue(ledger_path.exists())
        self.assertTrue(state_path.exists())
        ledger_events = [json.loads(line) for line in ledger_path.read_text(encoding="utf-8").splitlines() if line.strip()]
        event_names = [event["event"] for event in ledger_events]
        self.assertIn("run.started", event_names)
        self.assertIn("message.appended", event_names)
        self.assertEqual(event_names.count("step.finished"), 3)
        self.assertEqual(event_names.count("tool.call.requested"), 2)
        self.assertEqual(event_names.count("tool.call.started"), 2)
        self.assertEqual(event_names.count("tool.call.finished"), 2)
        self.assertIn("patch.detected", event_names)
        self.assertEqual(event_names.count("model.usage"), 3)
        self.assertIn("run.finished", event_names)

        summary = json.loads((ledger_path.parent / "summary.json").read_text(encoding="utf-8"))
        self.assertEqual(summary["status"], "completed")
        self.assertEqual(summary["step_count"], 3)
        self.assertEqual(summary["tool_call_count"], 2)
        self.assertEqual(summary["total_input_tokens"], 12)
        self.assertEqual(summary["total_output_tokens"], 15)

        restored = FileSessionStore(metadata["root_dir"]).load_session(session.id)
        self.assertEqual(restored.id, session.id)
        self.assertEqual([message.role for message in restored.messages], [message.role for message in session.messages])
        self.assertEqual(restored.messages[-1].content, "done")
        self.assertEqual(restored.metadata[SESSION_STORE_METADATA_KEY]["ledger_path"], metadata["ledger_path"])


if __name__ == "__main__":
    unittest.main()
