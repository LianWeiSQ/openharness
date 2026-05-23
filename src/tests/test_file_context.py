from __future__ import annotations

import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.file_context import (
    FILE_CONTEXT_METADATA_KEY,
    FileContextState,
    record_file_read,
)


class FileContextStateTests(unittest.TestCase):
    def _make_temp_dir(self) -> Path:
        root = Path("openagent/tests/file-context") / f"t_{uuid4().hex}"
        root.mkdir(parents=True, exist_ok=True)
        self.addCleanup(shutil.rmtree, root, True)
        return root

    def test_empty_metadata_returns_empty_state(self) -> None:
        state = FileContextState.from_metadata({})

        self.assertEqual(state.records, {})

    def test_record_read_stores_file_metadata(self) -> None:
        root = self._make_temp_dir()
        target = root / "a.py"
        target.write_text("print('hi')\n", encoding="utf-8")

        state = FileContextState()
        record = state.record_read("a.py", workspace_root=root, source_tool="read", now_ms=123)

        self.assertEqual(record.path, "a.py")
        self.assertEqual(record.source_tool, "read")
        self.assertEqual(record.read_at_ms, 123)
        self.assertIn("print", record.preview)
        self.assertEqual(len(record.content_hash), 64)

    def test_state_round_trips_through_metadata(self) -> None:
        root = self._make_temp_dir()
        target = root / "a.txt"
        target.write_text("alpha", encoding="utf-8")
        metadata = {}

        record_file_read(metadata, target, workspace_root=root, now_ms=1)
        loaded = FileContextState.from_metadata(metadata)

        self.assertIn(str(target.resolve()), loaded.records)
        self.assertEqual(loaded.records[str(target.resolve())].path, "a.txt")

    def test_invalid_metadata_records_are_skipped(self) -> None:
        metadata = {FILE_CONTEXT_METADATA_KEY: {"records": {"bad": {"path": "x"}}}}

        state = FileContextState.from_metadata(metadata)

        self.assertEqual(state.records, {})

    def test_change_for_unchanged_file(self) -> None:
        root = self._make_temp_dir()
        target = root / "a.txt"
        target.write_text("alpha", encoding="utf-8")
        state = FileContextState()
        record = state.record_read(target, workspace_root=root)

        change = state.change_for(record.absolute_path)

        self.assertIsNotNone(change)
        self.assertFalse(change.changed)
        self.assertEqual(change.reason, "unchanged")

    def test_change_for_modified_file(self) -> None:
        root = self._make_temp_dir()
        target = root / "a.txt"
        target.write_text("alpha", encoding="utf-8")
        state = FileContextState()
        record = state.record_read(target, workspace_root=root)
        target.write_text("alpha changed", encoding="utf-8")

        change = state.change_for(record.absolute_path)

        self.assertIsNotNone(change)
        self.assertTrue(change.changed)
        self.assertIn(change.reason, {"size", "hash"})

    def test_change_for_missing_file(self) -> None:
        root = self._make_temp_dir()
        target = root / "a.txt"
        target.write_text("alpha", encoding="utf-8")
        state = FileContextState()
        record = state.record_read(target, workspace_root=root)
        target.unlink()

        change = state.change_for(record.absolute_path)

        self.assertIsNotNone(change)
        self.assertTrue(change.changed)
        self.assertFalse(change.exists)
        self.assertEqual(change.reason, "missing")

    def test_changed_records_returns_only_changed_files(self) -> None:
        root = self._make_temp_dir()
        changed = root / "changed.txt"
        stable = root / "stable.txt"
        changed.write_text("old", encoding="utf-8")
        stable.write_text("same", encoding="utf-8")
        state = FileContextState()
        changed_record = state.record_read(changed, workspace_root=root)
        state.record_read(stable, workspace_root=root)
        changed.write_text("new", encoding="utf-8")

        changes = state.changed_records()

        self.assertEqual([item.record.absolute_path for item in changes], [changed_record.absolute_path])

    def test_to_context_items_uses_recent_order_and_file_kind(self) -> None:
        root = self._make_temp_dir()
        older = root / "older.txt"
        newer = root / "newer.txt"
        older.write_text("old preview", encoding="utf-8")
        newer.write_text("new preview", encoding="utf-8")
        state = FileContextState()
        state.record_read(older, workspace_root=root, now_ms=1)
        state.record_read(newer, workspace_root=root, now_ms=2)

        items = state.to_context_items(max_items=1)

        self.assertEqual(len(items), 1)
        self.assertEqual(items[0].kind, "file")
        self.assertEqual(items[0].priority, 70)
        self.assertIn("newer.txt", items[0].content)
        self.assertIn("new preview", items[0].content)

    def test_record_read_uses_provided_content_for_hash_and_preview(self) -> None:
        root = self._make_temp_dir()
        target = root / "a.txt"
        target.write_text("disk content", encoding="utf-8")

        state = FileContextState()
        record = state.record_read(target, workspace_root=root, content="model saw this", preview_chars=5)

        self.assertEqual(record.preview, "model")
        self.assertNotEqual(record.content_hash, FileContextState().record_read(target, workspace_root=root).content_hash)


if __name__ == "__main__":
    unittest.main()
