from __future__ import annotations

import json
import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.observability import (
    ObservationRecorder,
    TraceRecord,
    input_preview,
    sanitize_observation_value,
)


class ObservabilityTests(unittest.TestCase):
    def _make_temp_dir(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = root / f"observability_{uuid4().hex}"
        path.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    def test_recorder_writes_metadata_and_ring_buffer(self) -> None:
        metadata: dict[str, object] = {}
        recorder = ObservationRecorder.for_session(
            session_id="session-1",
            session_metadata=metadata,
            agent_name="agent",
            model_id="model",
            provider_id="provider",
            workspace="/tmp/work",
            options={"observability": {"max_events": 2}},
            base_dir=self._make_temp_dir(),
        )

        recorder.event("first")
        recorder.event("second")
        recorder.event("third")

        root = metadata["observability"]
        self.assertIsInstance(root, dict)
        self.assertEqual(root["event_count"], 3)
        events = root["events"]
        self.assertEqual([event["name"] for event in events], ["second", "third"])

    def test_recorder_writes_jsonl(self) -> None:
        temp = self._make_temp_dir()
        metadata: dict[str, object] = {}
        recorder = ObservationRecorder.for_session(
            session_id="session-1",
            session_metadata=metadata,
            agent_name="agent",
            model_id="model",
            provider_id="provider",
            workspace=str(temp),
            options={"observability": {"jsonl": True, "jsonl_dir": "obs"}},
            base_dir=temp,
        )

        recorder.event("sample", attributes={"value": 1})

        path = Path(metadata["observability"]["jsonl_path"])
        self.assertTrue(path.exists())
        lines = path.read_text(encoding="utf-8").splitlines()
        self.assertEqual(len(lines), 1)
        self.assertEqual(json.loads(lines[0])["name"], "sample")

    def test_sanitizer_redacts_sensitive_fields_and_preview_truncates(self) -> None:
        sanitized = sanitize_observation_value(
            {
                "api_key": "secret",
                "nested": {"password": "pw", "ok": "value"},
            }
        )

        self.assertEqual(sanitized["api_key"], "[redacted]")
        self.assertEqual(sanitized["nested"]["password"], "[redacted]")
        self.assertEqual(sanitized["nested"]["ok"], "value")
        self.assertIn("truncated", input_preview({"query": "x" * 100}, max_chars=32))

    def test_span_records_duration(self) -> None:
        metadata: dict[str, object] = {}
        trace = TraceRecord(
            trace_id="trace-1",
            session_id="session-1",
            run_id="run-1",
            agent_name="agent",
        )
        recorder = ObservationRecorder(trace=trace, session_metadata=metadata, base_dir=self._make_temp_dir())

        with recorder.span("model.call", kind="model") as span:
            span.set_attribute("input_tokens", 1)

        events = metadata["observability"]["events"]
        self.assertEqual(events[0]["name"], "model.call.started")
        self.assertEqual(events[1]["name"], "model.call.finished")
        self.assertGreaterEqual(events[1]["duration_ms"], 0)

    def test_disabled_config_does_not_record_events(self) -> None:
        metadata: dict[str, object] = {}
        recorder = ObservationRecorder.for_session(
            session_id="session-1",
            session_metadata=metadata,
            agent_name="agent",
            model_id=None,
            provider_id=None,
            workspace=None,
            options={"observability": {"enabled": False}},
            base_dir=self._make_temp_dir(),
        )

        recorder.event("sample")

        self.assertNotIn("observability", metadata)


if __name__ == "__main__":
    unittest.main()
