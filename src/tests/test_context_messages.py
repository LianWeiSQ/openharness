from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.context_messages import (
    build_messages_for_model,
    project_tool_result_to_message,
    prune_old_tool_messages,
)
from openagent.core.types import ChatMessage, ToolResult


class ContextMessagesTests(unittest.TestCase):
    def _make_temp_root(self) -> Path:
        tmp_root = Path("openagent/tests/workdir")
        tmp_root.mkdir(parents=True, exist_ok=True)
        root = (tmp_root / f"t_{uuid4().hex}").resolve()
        root.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, root, True)
        return root

    def test_project_tool_result_to_message_writes_full_output_when_preview_is_too_large(self) -> None:
        root = self._make_temp_root()
        result = ToolResult(
            call_id="tool-1",
            output="alpha\n" * 3000,
            metadata={"title": "Search", "count": 12, "truncated": True},
        )

        updated_result, tool_message = project_tool_result_to_message(
            result=result,
            tool_name="code_search",
            session_root=root,
            preview_bytes=256,
            preview_lines=5,
            line_max_chars=40,
        )

        self.assertIn("[Tool result] code_search", tool_message.content)
        self.assertIn("title=Search", tool_message.content)
        self.assertIn("status=ok", tool_message.content)
        self.assertIn("preview:", tool_message.content)
        self.assertIn("full_output=", tool_message.content)
        output_path = Path(updated_result.metadata["output_path"])
        self.assertTrue(output_path.exists())
        self.assertEqual(output_path.read_text(encoding="utf-8"), result.output)

    def test_prune_old_tool_messages_only_prunes_outside_recent_turn_window(self) -> None:
        old_tool = ChatMessage(
            role="tool",
            name="grep",
            tool_call_id="c1",
            content="x" * 8000,
            metadata={"title": "Old search", "count": 1, "truncated": True, "output_path": "old.txt"},
        )
        recent_tool = ChatMessage(
            role="tool",
            name="grep",
            tool_call_id="c2",
            content="y" * 8000,
            metadata={"title": "Recent search", "count": 1, "truncated": True, "output_path": "recent.txt"},
        )
        messages = [
            ChatMessage(role="user", content="old question"),
            old_tool,
            ChatMessage(role="assistant", content="working"),
            ChatMessage(role="user", content="recent question"),
            recent_tool,
            ChatMessage(role="user", content="current question"),
        ]

        pruned, reclaimed = prune_old_tool_messages(
            messages,
            bytes_per_token=1,
            keep_recent_user_turns=2,
            protect_input_tokens=0,
            min_input_tokens=1,
        )

        self.assertGreater(reclaimed, 0)
        self.assertIn("[Old tool result content cleared]", pruned[1].content)
        self.assertTrue(pruned[1].metadata["compacted"])
        self.assertEqual(pruned[4].content, recent_tool.content)

    def test_build_messages_for_model_injects_compaction_summary(self) -> None:
        messages = [
            ChatMessage(role="user", content="older"),
            ChatMessage(role="assistant", content="older answer"),
            ChatMessage(role="user", content="recent"),
        ]
        model_messages = build_messages_for_model(
            messages,
            {
                "context_compaction": {
                    "summary": "Goal: continue",
                    "compacted_until": 2,
                    "updated_at": 123,
                }
            },
        )

        self.assertEqual(model_messages[0].role, "assistant")
        self.assertIn("[Compacted context summary]", model_messages[0].content)
        self.assertEqual(model_messages[1:], messages[2:])

    def test_project_tool_result_to_message_prefers_metadata_preview(self) -> None:
        root = self._make_temp_root()
        result = ToolResult(
            call_id="tool-2",
            output="navigation noise\nmenu noise\nfooter noise",
            metadata={
                "title": "Page summary",
                "preview": "Executive summary\nMigration reached 68 percent completion.\nRelease candidate ships Friday.",
            },
        )

        updated_result, tool_message = project_tool_result_to_message(
            result=result,
            tool_name="web_fetch",
            session_root=root,
            preview_bytes=256,
            preview_lines=5,
            line_max_chars=80,
        )

        self.assertIn("Executive summary", tool_message.content)
        self.assertIn("68 percent completion", tool_message.content)
        self.assertIn("Release candidate", updated_result.metadata["context_preview"])
        self.assertNotIn("navigation noise", updated_result.metadata["context_preview"])
