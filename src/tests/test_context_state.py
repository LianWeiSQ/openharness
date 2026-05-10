from __future__ import annotations

import unittest

from openagent.core.context_state import (
    STRUCTURED_WORK_STATE_FORMAT,
    STRUCTURED_WORK_STATE_HEADER,
    build_compaction_record,
    parse_work_state_output,
    render_work_state,
)


class ContextStateTests(unittest.TestCase):
    def test_parse_json_work_state(self) -> None:
        parsed = parse_work_state_output(
            """
            {
              "task": "Implement structured compaction",
              "progress": ["Added design doc"],
              "decisions": ["Keep summary backward-compatible"],
              "files": [{"path": "src/openagent/core/context_state.py", "status": "created", "note": "schema parser"}],
              "tool_findings": ["Existing compaction used free-form summary"],
              "todos": ["Wire AgentLoop"],
              "open_questions": [],
              "blockers": [],
              "next_steps": ["Add tests"],
              "risks": ["Provider may return fenced JSON"]
            }
            """
        )

        self.assertEqual(parsed.source, "model_json")
        self.assertIsNone(parsed.parse_error)
        self.assertEqual(parsed.state["task"], "Implement structured compaction")
        self.assertEqual(parsed.state["files"][0]["status"], "created")
        self.assertIn(STRUCTURED_WORK_STATE_HEADER, parsed.summary)
        self.assertIn("src/openagent/core/context_state.py (created)", parsed.summary)

    def test_parse_fenced_json_work_state(self) -> None:
        parsed = parse_work_state_output(
            """Sure.
            ```json
            {"task": "Continue fix", "progress": ["Read failing test"], "next_steps": ["Patch parser"]}
            ```
            """
        )

        self.assertEqual(parsed.source, "model_json")
        self.assertEqual(parsed.state["task"], "Continue fix")
        self.assertIn("Read failing test", parsed.summary)

    def test_legacy_text_fallback_is_structured(self) -> None:
        parsed = parse_work_state_output("Goal: continue implementing the context feature")

        self.assertEqual(parsed.source, "legacy_text_fallback")
        self.assertIsNotNone(parsed.parse_error)
        self.assertEqual(parsed.state["task"], "Goal: continue implementing the context feature")
        self.assertIn(STRUCTURED_WORK_STATE_HEADER, parsed.summary)

    def test_build_compaction_record_keeps_compatibility_summary(self) -> None:
        record = build_compaction_record(
            raw_text='{"task":"Ship structured state","progress":["Parser works"]}',
            compacted_until=4,
            updated_at=123,
        )

        self.assertEqual(record["schema_version"], 1)
        self.assertEqual(record["format"], STRUCTURED_WORK_STATE_FORMAT)
        self.assertEqual(record["compacted_until"], 4)
        self.assertEqual(record["updated_at"], 123)
        self.assertIn("summary", record)
        self.assertIn("state", record)
        self.assertIn("Parser works", record["summary"])

    def test_render_work_state_omits_empty_sections(self) -> None:
        rendered = render_work_state(
            {
                "task": "Continue work",
                "progress": [],
                "files": [{"path": "a.py", "status": "unexpected", "note": "important"}],
            }
        )

        self.assertIn("Task:\nContinue work", rendered)
        self.assertIn("a.py (unknown) - important", rendered)
        self.assertNotIn("Progress:", rendered)


if __name__ == "__main__":
    unittest.main()
