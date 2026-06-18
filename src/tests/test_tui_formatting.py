from __future__ import annotations

import hashlib
import shutil
import unittest
from dataclasses import dataclass, field
from pathlib import Path
from uuid import uuid4
from unittest.mock import patch

from openagent.app_server.protocol import AppEvent
from openagent.app_server.runtime import OpenAgentAppRuntime
from openagent.tui.app import _handle_key
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
    id: str = "turn_capture"
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


class InterruptRuntime(DummyRuntime):
    def __init__(self, *, workspace: Path) -> None:
        super().__init__(workspace=workspace)
        self.interrupted_turn_ids: list[str] = []

    def interrupt_turn(self, turn_id: str):
        self.interrupted_turn_ids.append(turn_id)
        return {"id": turn_id, "status": "interrupting"}


class ApprovalRuntime(InterruptRuntime):
    def __init__(self, *, workspace: Path) -> None:
        super().__init__(workspace=workspace)
        self.approvals: list[tuple[str, str, str]] = []

    def respond_approval(self, turn_id: str, request_id: str, action: str):
        self.approvals.append((turn_id, request_id, action))
        return {"method": "turn/approval_resolved"}


class PatchRevertRuntime(DummyRuntime):
    def __init__(self, *, workspace: Path) -> None:
        super().__init__(workspace=workspace)
        self.reverts: list[tuple[str, str, str]] = []
        self.turn: CapturingTurn | None = None

    def revert_patch(self, turn_id: str, patch_ref: str = "last", *, target: str = "all"):
        self.reverts.append((turn_id, patch_ref, target))
        event = AppEvent(
            sequence=len(self.turn.events) + 1 if self.turn is not None else 1,
            method="item/patch/reverted",
            params={
                "thread_id": "session_live",
                "turn_id": turn_id,
                "patch_hash": "hash_123",
                "target": target,
                "reverted": ["a.txt: restored"],
                "skipped": [],
            },
        )
        if self.turn is not None:
            self.turn.events.append(event)
        return event.to_dict()


class SessionRuntime(DummyRuntime):
    def __init__(self, *, workspace: Path) -> None:
        super().__init__(workspace=workspace)
        self.resumed: list[str] = []
        self.sessions = [
            {"id": "session_alpha123", "status": "completed", "message_count": 2},
            {"id": "session_beta456", "status": "idle", "message_count": 1},
        ]
        self.messages = {
            "session_alpha123": [
                {"role": "user", "content": "old task"},
                {"role": "assistant", "content": "old answer"},
            ]
        }

    def list_sessions(self):
        return list(self.sessions)

    def resume_session(self, session_id: str):
        self.resumed.append(session_id)
        return {"id": session_id}

    def get_session(self, session_id: str):
        return {"id": session_id, "messages": self.messages.get(session_id, [])}


class SelectorRuntime(DummyRuntime):
    def __init__(self, *, workspace: Path) -> None:
        super().__init__(workspace=workspace)
        self.last_options: dict[str, object] = {}

    def list_models(self):
        return [
            {"id": "alpha", "provider_id": "openai", "name": "Alpha", "variants": []},
            {"id": "beta", "provider_id": "openai", "name": "Beta", "variants": ["fast", "deep"]},
        ]

    def list_agents(self):
        return [{"id": "build"}, {"id": "plan"}, {"id": "explore"}]

    def start_turn(self, *, session_id: str, user_text: str, **kwargs):
        self.last_session_id = session_id
        self.last_user_text = user_text
        self.last_options = dict(kwargs)
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

    def test_formats_interrupt_events(self) -> None:
        requested = format_event(AppEvent(sequence=1, method="turn/interrupt_requested", params={}))
        interrupted = format_event(AppEvent(sequence=2, method="turn/interrupted", params={"status": "interrupted"}))

        self.assertEqual(requested[0].kind, "warning")
        self.assertIn("interrupt requested", requested[0].text)
        self.assertEqual(interrupted[0].kind, "status")
        self.assertEqual(interrupted[0].text, "turn interrupted")

    def test_formats_approval_events(self) -> None:
        requested = format_event(
            AppEvent(
                sequence=1,
                method="turn/approval_requested",
                params={
                    "approval": {
                        "request_id": "approval_1",
                        "tool_name": "write",
                        "tool_input": {"file_path": "a.txt"},
                    }
                },
            )
        )
        resolved = format_event(
            AppEvent(
                sequence=2,
                method="turn/approval_resolved",
                params={
                    "approval": {
                        "request_id": "approval_1",
                        "tool_name": "write",
                        "action": "deny",
                    }
                },
            )
        )

        self.assertEqual(requested[0].kind, "warning")
        self.assertIn("approval required: write", requested[0].text)
        self.assertEqual(resolved[0].kind, "warning")
        self.assertEqual(resolved[0].text, "approval deny: write")

    def test_formats_patch_diff_and_revert_events(self) -> None:
        patch = format_event(
            AppEvent(
                sequence=1,
                method="item/patch/detected",
                params={
                    "event_type": "patch",
                    "event": {
                        "type": "patch",
                        "hash": "abcdef1234567890",
                        "files": [
                            {
                                "path": "a.txt",
                                "status": "modified",
                                "diff": "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new",
                            }
                        ],
                    },
                },
            )
        )
        reverted = format_event(
            AppEvent(
                sequence=2,
                method="item/patch/reverted",
                params={"patch_hash": "abcdef1234567890", "reverted": ["a.txt: restored"], "skipped": []},
            )
        )
        failed = format_event(
            AppEvent(
                sequence=3,
                method="item/patch/revert_failed",
                params={"patch_ref": "last", "error": "current content changed after patch"},
            )
        )

        self.assertEqual(patch[0].kind, "patch")
        self.assertIn("1. modified a.txt (+1/-1)", patch[0].text)
        self.assertIn("@@ -1 +1 @@", patch[0].text)
        self.assertIn("+new", patch[0].text)
        self.assertEqual(reverted[0].kind, "patch")
        self.assertIn("patch reverted: 1 item", reverted[0].text)
        self.assertEqual(failed[0].kind, "error")
        self.assertIn("current content changed after patch", failed[0].text)

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
        self.assertIn("/sessions - open recent session picker", timeline_text)
        self.assertIn("/transcript [limit] - show recent messages from the current session", timeline_text)
        self.assertIn("/review - Review changes", timeline_text)
        self.assertEqual(state.status, "commands listed")
        self.assertEqual(state.input_buffer, "")

    def test_tui_builtin_help_and_status_are_local(self) -> None:
        workspace = self._make_temp_dir()
        state = TuiState(runtime=DummyRuntime(workspace=workspace))  # type: ignore[arg-type]
        state.session_id = "session_live"
        state.input_buffer = "/help"

        self.assertFalse(state.submit())
        self.assertEqual(state.status, "help listed")
        self.assertIn("/resume <id>", "\n".join(line.text for line in state.timeline))
        self.assertIn("/transcript [limit]", "\n".join(line.text for line in state.timeline))

        state.input_buffer = "/status"
        self.assertFalse(state.submit())
        timeline_text = "\n".join(line.text for line in state.timeline)
        self.assertIn("session: session_live", timeline_text)
        self.assertIn("model: auto", timeline_text)
        self.assertIn("agent: build", timeline_text)
        self.assertIn(f"workspace: {workspace}", timeline_text)

    def test_tui_model_agent_variant_pickers_affect_next_turn(self) -> None:
        workspace = self._make_temp_dir()
        runtime = SelectorRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]

        state.input_buffer = "/models"
        self.assertFalse(state.submit())
        self.assertTrue(state.model_picker_open)
        self.assertFalse(_handle_key(ord("j"), state))
        self.assertFalse(_handle_key(10, state))
        self.assertFalse(state.model_picker_open)
        self.assertEqual(state.selected_model_id, "beta")
        self.assertEqual(state.selected_provider_id, "openai")

        state.input_buffer = "/agents"
        self.assertFalse(state.submit())
        self.assertTrue(state.agent_picker_open)
        self.assertFalse(_handle_key(ord("j"), state))
        self.assertFalse(_handle_key(10, state))
        self.assertEqual(state.selected_agent, "plan")

        state.input_buffer = "/variants"
        self.assertFalse(state.submit())
        self.assertTrue(state.variant_picker_open)
        self.assertFalse(_handle_key(ord("j"), state))
        self.assertFalse(_handle_key(10, state))
        self.assertEqual(state.selected_variant, "fast")

        state.input_buffer = "use selected runtime"
        self.assertTrue(state.submit())

        self.assertEqual(runtime.last_options["model_id"], "beta")
        self.assertEqual(runtime.last_options["provider_id"], "openai")
        self.assertEqual(runtime.last_options["agent_name"], "plan")
        self.assertEqual(runtime.last_options["variant"], "fast")
        timeline_text = "\n".join(line.text for line in state.timeline)
        self.assertIn("model selected: openai/beta", timeline_text)
        self.assertIn("agent selected: plan", timeline_text)
        self.assertIn("variant selected: fast", timeline_text)

    def test_tui_model_agent_variant_control_actions(self) -> None:
        workspace = self._make_temp_dir()
        runtime = SelectorRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]

        model_open = state.apply_control_request({"path": "/tui/open-models", "body": {}})
        model_select = state.apply_control_request({"path": "/tui/select-model", "body": {"modelID": "beta", "providerID": "openai"}})
        agent_open = state.apply_control_request({"path": "/tui/open-agents", "body": {}})
        agent_select = state.apply_control_request({"path": "/tui/select-agent", "body": {"agent": "explore"}})
        variant_open = state.apply_control_request({"path": "/tui/open-variants", "body": {}})
        variant_select = state.apply_control_request({"path": "/tui/select-variant", "body": {"variant": "deep"}})

        self.assertTrue(model_open["applied"])
        self.assertTrue(model_select["applied"])
        self.assertTrue(agent_open["applied"])
        self.assertTrue(agent_select["applied"])
        self.assertTrue(variant_open["applied"])
        self.assertTrue(variant_select["applied"])
        self.assertEqual(state.model_label, "openai/beta")
        self.assertEqual(state.agent_label, "explore")
        self.assertEqual(state.variant_label, "deep")

    def test_tui_sessions_and_resume_by_prefix(self) -> None:
        workspace = self._make_temp_dir()
        runtime = SessionRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.input_buffer = "/sessions"

        self.assertFalse(state.submit())
        self.assertTrue(state.session_picker_open)
        self.assertEqual(state.status, "session picker")
        self.assertEqual(state.selected_session(), runtime.sessions[0])

        state.input_buffer = "/resume session_alpha"
        self.assertFalse(state.submit())

        self.assertEqual(state.session_id, "session_alpha123")
        self.assertEqual(runtime.resumed, ["session_alpha123"])
        timeline_text = "\n".join(line.text for line in state.timeline)
        self.assertIn("> old task", timeline_text)
        self.assertIn("old answer", timeline_text)
        self.assertIn("resumed session: session_alpha123", timeline_text)

    def test_tui_transcript_renders_current_session_with_limit(self) -> None:
        workspace = self._make_temp_dir()
        runtime = SessionRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.session_id = "session_alpha123"
        state.input_buffer = "/transcript 1"

        self.assertFalse(state.submit())

        self.assertEqual(state.status, "transcript shown")
        timeline_text = "\n".join(line.text for line in state.timeline)
        self.assertIn("transcript: session_alpha123 (last 1 of 2 messages)", timeline_text)
        self.assertNotIn("> old task", timeline_text)
        self.assertIn("old answer", timeline_text)

    def test_tui_transcript_reports_unsupported_runtime(self) -> None:
        workspace = self._make_temp_dir()
        state = TuiState(runtime=DummyRuntime(workspace=workspace))  # type: ignore[arg-type]
        state.session_id = "session_live"
        state.input_buffer = "/transcript"

        self.assertFalse(state.submit())

        self.assertEqual(state.status, "transcript unsupported")
        self.assertIn("transcript is not supported by this runtime", "\n".join(line.text for line in state.timeline))

    def test_tui_diff_and_local_revert_commands_use_latest_patch(self) -> None:
        workspace = self._make_temp_dir()
        (workspace / "a.txt").write_text("new\n", encoding="utf-8")
        state = TuiState(runtime=DummyRuntime(workspace=workspace))  # type: ignore[arg-type]
        state.patch_records = [
            {
                "hash": "hash_123",
                "files": [
                    {
                        "path": "a.txt",
                        "status": "modified",
                        "diff": "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new",
                        "before_text": "old\n",
                        "after_text": "new\n",
                        "before_sha256": hashlib.sha256(b"old\n").hexdigest(),
                        "after_sha256": hashlib.sha256(b"new\n").hexdigest(),
                        "text_available": True,
                    }
                ],
            }
        ]

        state.input_buffer = "/diff"
        self.assertFalse(state.submit())
        self.assertEqual(state.status, "diff shown")
        self.assertIn("+new", "\n".join(line.text for line in state.timeline))

        state.input_buffer = "/revert"
        self.assertFalse(state.submit())

        self.assertEqual((workspace / "a.txt").read_text(encoding="utf-8"), "old\n")
        self.assertEqual(state.status, "revert complete")
        self.assertIn("a.txt: restored", "\n".join(line.text for line in state.timeline))

    def test_tui_revert_prefers_runtime_when_active_turn_exists(self) -> None:
        workspace = self._make_temp_dir()
        runtime = PatchRevertRuntime(workspace=workspace)
        patch_event = AppEvent(
            sequence=1,
            method="item/patch/detected",
            params={
                "event_type": "patch",
                "event": {
                    "type": "patch",
                    "hash": "hash_123",
                    "files": [
                        {
                            "path": "a.txt",
                            "status": "modified",
                            "diff": "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new",
                            "before_text": "old\n",
                            "after_text": "new\n",
                            "text_available": True,
                        }
                    ],
                },
            },
        )
        turn = CapturingTurn(status="completed", id="turn_live", events=[patch_event])
        runtime.turn = turn
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.active_turn = turn
        state.poll_events()
        state.input_buffer = "/revert 1"

        self.assertFalse(state.submit())

        self.assertEqual(runtime.reverts, [("turn_live", "last", "1")])
        self.assertEqual(state.status, "revert complete")
        self.assertIn("patch reverted: 1 item", "\n".join(line.text for line in state.timeline))

    def test_tui_session_picker_keyboard_resumes_selected_session(self) -> None:
        workspace = self._make_temp_dir()
        runtime = SessionRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]

        self.assertFalse(_handle_key(18, state))  # Ctrl-R
        self.assertTrue(state.session_picker_open)
        self.assertEqual(state.selected_session(), runtime.sessions[0])

        self.assertFalse(_handle_key(ord("j"), state))
        self.assertEqual(state.selected_session(), runtime.sessions[1])

        self.assertFalse(_handle_key(ord("k"), state))
        self.assertEqual(state.selected_session(), runtime.sessions[0])

        self.assertFalse(_handle_key(ord("j"), state))
        self.assertFalse(_handle_key(10, state))  # Enter

        self.assertFalse(state.session_picker_open)
        self.assertEqual(state.session_id, "session_beta456")
        self.assertEqual(runtime.resumed, ["session_beta456"])
        self.assertIn("resumed session: session_beta456", "\n".join(line.text for line in state.timeline))

    def test_tui_session_picker_escape_closes_without_resuming(self) -> None:
        workspace = self._make_temp_dir()
        runtime = SessionRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]

        self.assertTrue(state.open_session_picker())
        self.assertFalse(_handle_key(27, state))  # Esc

        self.assertFalse(state.session_picker_open)
        self.assertEqual(state.status, "session picker closed")
        self.assertEqual(runtime.resumed, [])

    def test_tui_request_interrupt_calls_runtime(self) -> None:
        workspace = self._make_temp_dir()
        runtime = InterruptRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.active_turn = CapturingTurn(status="running", id="turn_live")

        state.request_interrupt()

        self.assertEqual(runtime.interrupted_turn_ids, ["turn_live"])
        self.assertEqual(state.status, "interrupting")

    def test_tui_approval_keyboard_calls_runtime(self) -> None:
        workspace = self._make_temp_dir()
        runtime = ApprovalRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.active_turn = CapturingTurn(
            status="waiting_approval",
            id="turn_live",
            events=[
                AppEvent(
                    sequence=1,
                    method="turn/approval_requested",
                    params={
                        "approval": {
                            "request_id": "approval_1",
                            "turn_id": "turn_live",
                            "tool_name": "write",
                            "tool_input": {"file_path": "a.txt"},
                        }
                    },
                )
            ],
        )

        state.poll_events()
        self.assertEqual(state.status, "approval required")
        self.assertEqual(state.active_approval["request_id"], "approval_1")
        self.assertFalse(_handle_key(ord("a"), state))

        self.assertEqual(runtime.approvals, [("turn_live", "approval_1", "allow")])
        self.assertIsNone(state.active_approval)
        self.assertEqual(state.status, "approval allow sent")

    def test_tui_approval_ctrl_c_denies_and_interrupts(self) -> None:
        workspace = self._make_temp_dir()
        runtime = ApprovalRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.active_turn = CapturingTurn(status="waiting_approval", id="turn_live")
        state.active_approval = {
            "request_id": "approval_1",
            "turn_id": "turn_live",
            "tool_name": "write",
            "tool_input": {"file_path": "a.txt"},
        }

        self.assertFalse(_handle_key(3, state))

        self.assertEqual(runtime.approvals, [("turn_live", "approval_1", "deny")])
        self.assertEqual(runtime.interrupted_turn_ids, ["turn_live"])
        self.assertEqual(state.status, "interrupting")

    def test_tui_resume_reports_ambiguous_prefix(self) -> None:
        workspace = self._make_temp_dir()
        runtime = SessionRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.input_buffer = "/resume session_"

        self.assertFalse(state.submit())

        self.assertEqual(state.status, "session ambiguous")
        self.assertEqual(runtime.resumed, [])
        timeline_text = "\n".join(line.text for line in state.timeline)
        self.assertIn("session prefix is ambiguous", timeline_text)
        self.assertIn("session_alpha123", timeline_text)

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

    def test_tui_file_picker_inserts_file_mention(self) -> None:
        workspace = self._make_temp_dir()
        (workspace / "README.md").write_text("hello", encoding="utf-8")
        (workspace / "src").mkdir()
        (workspace / "src" / "main.py").write_text("print('hi')", encoding="utf-8")
        runtime = CapturingRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.input_buffer = "review "

        for char in "@read":
            self.assertFalse(_handle_key(ord(char), state))

        self.assertTrue(state.file_picker_open)
        self.assertEqual(state.selected_file_mention(), "README.md")
        self.assertFalse(_handle_key(9, state))  # Tab inserts selected file.

        self.assertFalse(state.file_picker_open)
        self.assertEqual(state.input_buffer, "review @README.md ")
        self.assertEqual(state.status, "inserted @README.md")

    def test_tui_plain_prompt_expands_file_mentions(self) -> None:
        workspace = self._make_temp_dir()
        (workspace / "README.md").write_text("file context", encoding="utf-8")
        runtime = CapturingRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]
        state.input_buffer = "review @README.md"

        self.assertTrue(state.submit())

        self.assertIn("Attached file:", runtime.last_user_text or "")
        self.assertIn("file context", runtime.last_user_text or "")
        self.assertIn("> review @README.md", "\n".join(line.text for line in state.timeline))

    def test_tui_state_applies_prompt_session_toast_and_publish_controls(self) -> None:
        workspace = self._make_temp_dir()
        runtime = SessionRuntime(workspace=workspace)
        state = TuiState(runtime=runtime)  # type: ignore[arg-type]

        append_result = state.apply_control_request({"path": "/tui/append-prompt", "body": {"text": "hello"}})
        clear_result = state.apply_control_request({"path": "/tui/clear-prompt", "body": {}})
        help_result = state.apply_control_request({"path": "/tui/open-help", "body": {}})
        sessions_result = state.apply_control_request({"path": "/tui/open-sessions", "body": {}})
        select_result = state.apply_control_request({"path": "/tui/select-session", "body": {"sessionID": "session_alpha123"}})
        toast_result = state.apply_control_request({"path": "/tui/show-toast", "body": {"title": "Saved", "message": "Session selected", "variant": "success"}})
        publish_result = state.apply_control_request({"path": "/tui/publish", "body": {"type": "tui.prompt.append", "properties": {"text": " next"}}})
        unsupported_result = state.apply_control_request({"path": "/tui/open-themes", "body": {}})

        self.assertEqual(append_result["applied"], True)
        self.assertEqual(clear_result["applied"], True)
        self.assertEqual(help_result["applied"], True)
        self.assertEqual(sessions_result["applied"], True)
        self.assertEqual(select_result["applied"], True)
        self.assertEqual(toast_result["applied"], True)
        self.assertEqual(publish_result["applied"], True)
        self.assertEqual(unsupported_result["unsupported"], True)
        self.assertEqual(state.session_id, "session_alpha123")
        self.assertEqual(state.input_buffer, " next")
        timeline_text = "\n".join(line.text for line in state.timeline)
        self.assertIn("resumed session: session_alpha123", timeline_text)
        self.assertIn("Saved: Session selected", timeline_text)
        self.assertIn("TUI control unsupported: theme.open", timeline_text)

    def test_tui_state_control_execute_command_uses_slash_commands(self) -> None:
        workspace = self._make_temp_dir()
        state = TuiState(runtime=DummyRuntime(workspace=workspace))  # type: ignore[arg-type]

        result = state.apply_control_request({"path": "/tui/execute-command", "body": {"command": "help"}})

        self.assertEqual(result["applied"], False)
        self.assertEqual(state.input_buffer, "")
        self.assertEqual(state.status, "help listed")
        self.assertIn("/sessions - open recent session picker", "\n".join(line.text for line in state.timeline))

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
