from __future__ import annotations

import shutil
import unittest
from dataclasses import dataclass, field
from pathlib import Path
from uuid import uuid4
from unittest.mock import patch

from openagent.app_server.protocol import AppEvent
from openagent.app_server.runtime import OpenAgentAppRuntime
from openagent.tui.formatting import format_event, short_id, trace_label, wrap_lines
from openagent.tui.state import TuiState

from _mock_model import ScriptedLanguageModel


class DummyRuntime:
    def __init__(self, *, workspace: Path | None = None) -> None:
        self.session_count = 0
        self.workspace = workspace or Path.cwd()

    def start_session(self):
        self.session_count += 1
        return {"id": f"session_{self.session_count}"}

    def start_turn(self, *, session_id: str, user_text: str):
        raise AssertionError("not used")


@dataclass(slots=True)
class CapturingTurn:
    status: str = "completed"
    events: list[AppEvent] = field(default_factory=list)


class CapturingRuntime(DummyRuntime):
    def __init__(self, *, workspace: Path) -> None:
        super().__init__(workspace=workspace)
        self.last_session_id: str | None = None
        self.last_user_text: str | None = None

    def start_turn(self, *, session_id: str, user_text: str):
        self.last_session_id = session_id
        self.last_user_text = user_text
        return CapturingTurn()


class TuiFormattingTests(unittest.TestCase):
    def _make_temp_dir(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        td = tmp_root / f"tui_{uuid4().hex}"
        td.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, td, True)
        return td

    def test_formats_tool_call_event(self) -> None:
        event = AppEvent(
            sequence=1,
            method="item/toolCall/started",
            params={
                "event": {
                    "type": "tool-call",
                    "name": "ls",
                    "input": {"path": "."},
                    "call_id": "call_1",
                }
            },
        )

        lines = format_event(event)

        self.assertEqual(lines[0].kind, "tool")
        self.assertIn("tool call: ls", lines[0].text)

    def test_formats_completion_with_trace(self) -> None:
        event = AppEvent(
            sequence=1,
            method="turn/completed",
            params={
                "status": "completed",
                "final_answer": "done",
                "trace": {"trace_id": "trace_123"},
            },
        )

        lines = format_event(event)

        self.assertEqual([line.kind for line in lines], ["status", "assistant", "trace"])
        self.assertEqual(lines[1].text, "done")
        self.assertEqual(lines[2].text, "trace: trace_123")

    def test_helpers(self) -> None:
        self.assertEqual(short_id("abcdef", keep=10), "abcdef")
        self.assertEqual(short_id("abcdefghijkl", keep=4), "abcd...")
        self.assertEqual(trace_label({"run_id": "run_1"}), "run_1")
        self.assertEqual(len(wrap_lines(format_event(AppEvent(sequence=1, method="x", params={"a": "b"})), width=8)), 2)

    def test_state_starts_session(self) -> None:
        state = TuiState(runtime=DummyRuntime())  # type: ignore[arg-type]

        session_id = state.ensure_session()

        self.assertEqual(session_id, "session_1")
        self.assertEqual(state.ensure_session(), "session_1")

    def test_tui_slash_commands_lists_custom_commands(self) -> None:
        workspace = self._make_temp_dir()
        write_tui_command(workspace, "review", "---\ndescription: Review changes\n---\nReview $ARGUMENTS.")
        state = TuiState(runtime=DummyRuntime(workspace=workspace))  # type: ignore[arg-type]
        state.input_buffer = "/commands"

        with patch.dict("os.environ", {"HOME": str(workspace / "home")}):
            self.assertFalse(state.submit())

        timeline_text = "\n".join(line.text for line in state.timeline)
        self.assertIn("/review - Review changes", timeline_text)
        self.assertEqual(state.status, "commands listed")
        self.assertEqual(state.input_buffer, "")

    def test_tui_slash_command_renders_and_submits(self) -> None:
        workspace = self._make_temp_dir()
        (workspace / "note.txt").write_text("file context", encoding="utf-8")
        write_tui_command(
            workspace,
            "review",
            "---\ndescription: Review target\n---\nReview $1 / $ARGUMENTS and @note.txt.",
        )
        runtime = CapturingRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.input_buffer = "/review README"

        with patch.dict("os.environ", {"HOME": str(workspace / "home")}):
            self.assertTrue(state.submit())

        self.assertEqual(runtime.last_session_id, "session_1")
        self.assertIn("Review README / README", runtime.last_user_text or "")
        self.assertIn("file context", runtime.last_user_text or "")
        timeline_text = "\n".join(line.text for line in state.timeline)
        self.assertIn("slash command: /review", timeline_text)
        self.assertIn("> /review README", timeline_text)

    def test_tui_missing_slash_command_does_not_start_turn(self) -> None:
        workspace = self._make_temp_dir()
        runtime = CapturingRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.input_buffer = "/missing arg"

        with patch.dict("os.environ", {"HOME": str(workspace / "home")}):
            self.assertFalse(state.submit())

        self.assertIsNone(runtime.last_user_text)
        self.assertEqual(state.status, "command not found")
        self.assertIn("slash command not found", "\n".join(line.text for line in state.timeline))

    def test_tui_submit_runs_openagent_loop_and_tool_event(self) -> None:
        workspace = self._make_temp_dir()
        (workspace / "sample.txt").write_text("hello", encoding="utf-8")
        model = ScriptedLanguageModel(
            script=[
                [
                    {"type": "tool-call", "call_id": "call_1", "name": "ls", "input": {"path": "."}},
                    {
                        "type": "finish",
                        "finish_reason": "tool_call",
                        "usage": {"input_tokens": 2, "output_tokens": 1, "cost": 0.0},
                    },
                ],
                [
                    {"type": "text-delta", "id": "t1", "text": "I listed the workspace."},
                    {
                        "type": "finish",
                        "finish_reason": "stop",
                        "usage": {"input_tokens": 3, "output_tokens": 4, "cost": 0.0},
                    },
                ],
            ]
        )
        runtime = OpenAgentAppRuntime(
            workspace=workspace,
            session_store_root=workspace / ".openagent" / "sessions",
            language_model_factory=lambda _model: model,
        )
        state = TuiState(runtime=runtime)
        state.input_buffer = "list files"

        self.assertTrue(state.submit())
        assert state.active_turn is not None
        self.assertTrue(state.active_turn.wait_until_terminal(timeout_s=10.0))
        state.poll_events()

        timeline_text = "\n".join(line.text for line in state.timeline)
        methods = [event.method for event in state.active_turn.events]
        self.assertEqual(state.active_turn.status, "completed")
        self.assertIn("item/toolCall/started", methods)
        self.assertIn("item/toolCall/completed", methods)
        self.assertIn("turn/completed", methods)
        self.assertIn("> list files", timeline_text)
        self.assertIn("tool call: ls", timeline_text)
        self.assertIn("I listed the workspace.", timeline_text)


def write_tui_command(workspace: Path, name: str, content: str) -> Path:
    directory = workspace / ".openagent" / "commands"
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / f"{name}.md"
    path.write_text(content, encoding="utf-8")
    return path


if __name__ == "__main__":
    unittest.main()
