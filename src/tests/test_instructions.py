from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.instructions import InstructionContextLoader, InstructionLoadOptions


class InstructionContextLoaderTests(unittest.TestCase):
    def _make_temp_dir(self) -> Path:
        root = Path("openagent/tests/instructions") / f"t_{uuid4().hex}"
        root.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, root, True)
        return root

    def test_loads_workspace_openagent_file(self) -> None:
        root = self._make_temp_dir()
        (root / "OPENAGENT.md").write_text("Project rule", encoding="utf-8")

        context = InstructionContextLoader(root, InstructionLoadOptions(include_user=False)).load()

        self.assertEqual(len(context.items), 1)
        self.assertEqual(context.items[0].display_path, "OPENAGENT.md")
        self.assertIn("Project rule", context.items[0].content)

    def test_loads_closest_workspace_before_parent(self) -> None:
        root = self._make_temp_dir()
        child = root / "pkg" / "feature"
        child.mkdir(parents=True)
        (root / "AGENTS.md").write_text("Parent rule", encoding="utf-8")
        (child / "AGENTS.md").write_text("Child rule", encoding="utf-8")

        context = InstructionContextLoader(child, InstructionLoadOptions(include_user=False)).load()

        self.assertGreaterEqual(len(context.items), 2)
        self.assertIn("Child rule", context.items[0].content)
        self.assertIn("Parent rule", context.items[1].content)

    def test_loads_openagent_instructions_and_sorted_rules(self) -> None:
        root = self._make_temp_dir()
        rules = root / ".openagent" / "rules"
        rules.mkdir(parents=True)
        (root / ".openagent" / "instructions.md").write_text("Main instructions", encoding="utf-8")
        (rules / "b.md").write_text("Rule B", encoding="utf-8")
        (rules / "a.md").write_text("Rule A", encoding="utf-8")

        context = InstructionContextLoader(root, InstructionLoadOptions(include_user=False)).load()
        sources = [item.source for item in context.items]

        self.assertIn("instructions.workspace:.openagent/instructions.md", sources)
        self.assertLess(
            sources.index("instructions.workspace:.openagent/rules/a.md"),
            sources.index("instructions.workspace:.openagent/rules/b.md"),
        )

    def test_loads_user_instructions_from_config_dir(self) -> None:
        root = self._make_temp_dir()
        user_dir = root / "user"
        user_dir.mkdir()
        (user_dir / "OPENAGENT.md").write_text("User rule", encoding="utf-8")

        context = InstructionContextLoader(
            root,
            InstructionLoadOptions(user_config_dir=user_dir),
        ).load()

        self.assertEqual(len(context.items), 1)
        self.assertEqual(context.items[0].scope, "user")
        self.assertIn("User rule", context.items[0].content)

    def test_can_disable_user_instructions(self) -> None:
        root = self._make_temp_dir()
        user_dir = root / "user"
        user_dir.mkdir()
        (user_dir / "OPENAGENT.md").write_text("User rule", encoding="utf-8")

        context = InstructionContextLoader(
            root,
            InstructionLoadOptions(include_user=False, user_config_dir=user_dir),
        ).load()

        self.assertEqual(context.items, [])

    def test_enforces_max_file_bytes(self) -> None:
        root = self._make_temp_dir()
        (root / "OPENAGENT.md").write_text("abcdef", encoding="utf-8")

        context = InstructionContextLoader(
            root,
            InstructionLoadOptions(include_user=False, max_file_bytes=3),
        ).load()

        self.assertEqual(context.items[0].content, "abc")
        self.assertTrue(context.items[0].truncated)
        self.assertTrue(context.truncated)
        self.assertIn("truncated:OPENAGENT.md", context.issues)

    def test_enforces_total_bytes(self) -> None:
        root = self._make_temp_dir()
        (root / "OPENAGENT.md").write_text("abc", encoding="utf-8")
        (root / "AGENTS.md").write_text("def", encoding="utf-8")

        context = InstructionContextLoader(
            root,
            InstructionLoadOptions(include_user=False, max_total_bytes=4),
        ).load()

        self.assertEqual(context.total_bytes, 4)
        self.assertTrue(context.truncated)
        self.assertEqual([item.content for item in context.items], ["abc", "d"])

    def test_skips_binary_files(self) -> None:
        root = self._make_temp_dir()
        (root / "OPENAGENT.md").write_bytes(b"abc\x00def")

        context = InstructionContextLoader(root, InstructionLoadOptions(include_user=False)).load()

        self.assertEqual(context.items, [])
        self.assertIn("skipped_unreadable:OPENAGENT.md", context.issues)

    def test_items_convert_to_pinned_instruction_context_items(self) -> None:
        root = self._make_temp_dir()
        (root / "CLAUDE.md").write_text("Claude-compatible rule", encoding="utf-8")

        context = InstructionContextLoader(root, InstructionLoadOptions(include_user=False)).load()
        item = context.to_context_items()[0]

        self.assertEqual(item.kind, "instruction")
        self.assertEqual(item.priority, 100)
        self.assertTrue(item.pinned)
        self.assertTrue(item.stable_prefix)
        self.assertIn("Claude-compatible rule", item.content)


if __name__ == "__main__":
    unittest.main()
