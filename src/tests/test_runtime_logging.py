from __future__ import annotations

import json
import shutil
import unittest
from pathlib import Path
from uuid import uuid4

from openagent.core.runtime_logging import RuntimeLogger, load_runtime_logging_config


class RuntimeLoggingTests(unittest.TestCase):
    def _make_temp_dir(self) -> Path:
        root = Path("openagent/tests/workdir")
        root.mkdir(parents=True, exist_ok=True)
        path = root / f"runtime_logging_{uuid4().hex}"
        path.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    def test_logger_writes_metadata_and_ring_buffer(self) -> None:
        metadata: dict[str, object] = {}
        logger = RuntimeLogger.for_session(
            session_id="session-1",
            session_metadata=metadata,
            options={"logging": {"max_records": 2}},
            base_dir=self._make_temp_dir(),
            run_id="run-1",
            trace_id="trace-1",
        )

        logger.info("first")
        logger.warning("second")
        logger.error("third")

        root = metadata["runtime_logging"]
        self.assertIsInstance(root, dict)
        self.assertEqual(root["record_count"], 3)
        self.assertEqual([record["message"] for record in root["records"]], ["second", "third"])
        self.assertEqual(root["run_id"], "run-1")
        self.assertEqual(root["trace_id"], "trace-1")

    def test_logger_writes_jsonl(self) -> None:
        temp = self._make_temp_dir()
        metadata: dict[str, object] = {}
        logger = RuntimeLogger.for_session(
            session_id="session-1",
            session_metadata=metadata,
            options={"logging": {"jsonl": True, "jsonl_dir": "logs"}},
            base_dir=temp,
            run_id="run-1",
            trace_id="trace-1",
        )

        logger.info("sample", attributes={"value": 1})

        path = Path(metadata["runtime_logging"]["jsonl_path"])
        self.assertTrue(path.exists())
        lines = path.read_text(encoding="utf-8").splitlines()
        self.assertEqual(len(lines), 1)
        payload = json.loads(lines[0])
        self.assertEqual(payload["message"], "sample")
        self.assertEqual(payload["trace_id"], "trace-1")

    def test_level_filter_and_sensitive_redaction(self) -> None:
        metadata: dict[str, object] = {}
        logger = RuntimeLogger.for_session(
            session_id="session-1",
            session_metadata=metadata,
            options={"logging": {"level": "WARNING"}},
            base_dir=self._make_temp_dir(),
        )

        logger.info("hidden")
        logger.warning("visible", attributes={"api_key": "secret", "input_tokens": 12})

        records = metadata["runtime_logging"]["records"]
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]["message"], "visible")
        self.assertEqual(records[0]["attributes"]["api_key"], "[redacted]")
        self.assertEqual(records[0]["attributes"]["input_tokens"], 12)

    def test_disabled_config_does_not_record_metadata(self) -> None:
        metadata: dict[str, object] = {}
        logger = RuntimeLogger.for_session(
            session_id="session-1",
            session_metadata=metadata,
            options={"logging": {"enabled": False}},
            base_dir=self._make_temp_dir(),
        )

        logger.error("hidden")

        self.assertNotIn("runtime_logging", metadata)

    def test_config_parses_string_level_and_jsonl_options(self) -> None:
        config = load_runtime_logging_config(
            {
                "logging": {
                    "enabled": "true",
                    "jsonl": "yes",
                    "level": "debug",
                    "input_preview_chars": 16,
                }
            }
        )

        self.assertTrue(config.enabled)
        self.assertTrue(config.jsonl)
        self.assertEqual(config.level, "DEBUG")
        self.assertEqual(config.input_preview_chars, 16)


if __name__ == "__main__":
    unittest.main()
