from __future__ import annotations

import unittest

from openagent.core.context_pack import (
    ContextItem,
    ContextPackBuildOptions,
    ContextPackBuilder,
    estimate_text_tokens,
)
from openagent.core.session.todo import TodoItem
from openagent.core.types import ChatMessage


class ContextPackTests(unittest.TestCase):
    def test_estimate_text_tokens_uses_byte_ceiling(self) -> None:
        self.assertEqual(estimate_text_tokens("abcd", bytes_per_token=3), 2)
        self.assertEqual(estimate_text_tokens("", bytes_per_token=3), 1)

    def test_builder_collects_runtime_context(self) -> None:
        pack = ContextPackBuilder().build(messages=[], runtime_context="[Runtime context]\nNow")

        runtime = self._item(pack.items, "runtime:current")
        self.assertEqual(runtime.kind, "runtime")
        self.assertTrue(runtime.pinned)
        self.assertIn("Now", runtime.content)

    def test_builder_collects_structured_work_state_from_metadata(self) -> None:
        messages = [ChatMessage(role="user", content="old"), ChatMessage(role="user", content="new")]
        pack = ContextPackBuilder().build(
            messages=messages,
            metadata={
                "context_compaction": {
                    "schema_version": 1,
                    "format": "structured_work_state",
                    "state": {"task": "Continue context work", "next_steps": ["add builder"]},
                    "summary": "ignored",
                    "compacted_until": 1,
                    "updated_at": 1,
                }
            },
        )

        item = self._item(pack.items, "work_state:context_compaction")
        self.assertEqual(item.kind, "work_state")
        self.assertTrue(item.pinned)
        self.assertIn("Continue context work", item.content)

    def test_builder_skips_invalid_work_state_metadata(self) -> None:
        pack = ContextPackBuilder().build(
            messages=[ChatMessage(role="user", content="new")],
            metadata={"context_compaction": {"summary": "bad", "compacted_until": 99}},
        )

        self.assertNotIn("work_state:context_compaction", {item.id for item in pack.items})

    def test_builder_collects_todos(self) -> None:
        pack = ContextPackBuilder().build(
            messages=[],
            todos=[TodoItem(content="write tests", status="in_progress", priority="high", id="t1")],
        )

        item = self._item(pack.items, "todo:session")
        self.assertEqual(item.kind, "todo")
        self.assertIn("write tests", item.content)
        self.assertEqual(item.metadata["count"], 1)

    def test_builder_accepts_dict_todos(self) -> None:
        pack = ContextPackBuilder().build(messages=[], todos=[{"content": "ship docs", "status": "pending"}])

        self.assertIn("ship docs", self._item(pack.items, "todo:session").content)

    def test_builder_collects_regular_messages(self) -> None:
        pack = ContextPackBuilder().build(messages=[ChatMessage(role="user", content="hello")])

        item = self._item(pack.items, "message:user:0")
        self.assertEqual(item.kind, "message")
        self.assertEqual(item.priority, 40)

    def test_builder_collects_tool_result_messages(self) -> None:
        pack = ContextPackBuilder().build(
            messages=[ChatMessage(role="tool", content="preview", name="grep", tool_call_id="call-1")]
        )

        item = self._item(pack.items, "tool_result:call-1")
        self.assertEqual(item.kind, "tool_result")
        self.assertEqual(item.priority, 50)
        self.assertEqual(item.metadata["name"], "grep")

    def test_builder_collects_sandbox_metadata_without_connection_details(self) -> None:
        pack = ContextPackBuilder().build(
            messages=[],
            metadata={
                "execution": {
                    "mode": "opensandbox",
                    "sandbox_id": "sbx_1",
                    "remote_workdir": "/workspace/project",
                    "connection": {"token": "secret"},
                }
            },
        )

        item = self._item(pack.items, "sandbox:execution")
        self.assertEqual(item.kind, "sandbox")
        self.assertTrue(item.stable_prefix)
        self.assertIn("sbx_1", item.content)
        self.assertNotIn("secret", item.content)

    def test_builder_dedupes_items_and_keeps_higher_rank(self) -> None:
        pack = ContextPackBuilder().build(
            messages=[],
            extra_items=[
                ContextItem(id="x", kind="diagnostic", source="a", content="low", priority=1),
                ContextItem(id="x", kind="diagnostic", source="b", content="high", priority=9),
            ],
        )

        self.assertEqual(len([item for item in pack.items if item.id == "x"]), 1)
        self.assertEqual(self._item(pack.items, "x").content, "high")

    def test_builder_without_budget_includes_all_items(self) -> None:
        pack = ContextPackBuilder().build(
            messages=[ChatMessage(role="user", content="hello")],
            extra_items=[ContextItem(id="diag", kind="diagnostic", source="test", content="d", priority=1)],
        )

        self.assertTrue(all(entry.included for entry in pack.trace))

    def test_builder_budget_drops_lower_priority_unpinned_items(self) -> None:
        builder = ContextPackBuilder(ContextPackBuildOptions(token_budget=2, bytes_per_token=1))
        pack = builder.build(
            messages=[],
            extra_items=[
                ContextItem(id="a", kind="diagnostic", source="test", content="a", priority=10),
                ContextItem(id="b", kind="diagnostic", source="test", content="bbbb", priority=1),
            ],
        )

        trace = {entry.item_id: entry for entry in pack.trace}
        self.assertTrue(trace["a"].included)
        self.assertFalse(trace["b"].included)
        self.assertEqual(trace["b"].drop_reason, "budget")

    def test_builder_keeps_pinned_items_even_when_over_budget(self) -> None:
        builder = ContextPackBuilder(ContextPackBuildOptions(token_budget=1, bytes_per_token=1))
        pack = builder.build(
            messages=[],
            extra_items=[ContextItem(id="p", kind="diagnostic", source="test", content="too long", priority=1, pinned=True)],
        )

        self.assertTrue(pack.trace[0].included)

    def test_trace_only_preserves_original_messages(self) -> None:
        messages = [ChatMessage(role="user", content="hello")]
        pack = ContextPackBuilder().build(messages=messages, runtime_context="now")

        self.assertEqual(pack.messages, messages)

    def test_render_mode_returns_synthetic_context_messages(self) -> None:
        builder = ContextPackBuilder(ContextPackBuildOptions(trace_only=False))
        pack = builder.build(messages=[], runtime_context="now")

        self.assertEqual(len(pack.messages), 1)
        self.assertEqual(pack.messages[0].role, "assistant")
        self.assertTrue(pack.messages[0].metadata["synthetic_context_item"])

    def test_trace_dicts_are_json_ready(self) -> None:
        pack = ContextPackBuilder().build(messages=[], runtime_context="now")

        self.assertEqual(pack.trace_dicts()[0]["item_id"], "runtime:current")

    def _item(self, items, item_id: str):
        for item in items:
            if item.id == item_id:
                return item
        self.fail(f"Missing item: {item_id}")


if __name__ == "__main__":
    unittest.main()

