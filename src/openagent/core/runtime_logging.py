from __future__ import annotations

import json
import logging
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

from .id import new_id
from .observability import DEFAULT_FIELD_PREVIEW_CHARS, sanitize_observation_value

LOGGING_METADATA_KEY = "runtime_logging"
DEFAULT_JSONL_DIR = ".openagent/logs"
DEFAULT_MAX_RECORDS = 500
DEFAULT_INPUT_PREVIEW_CHARS = 2048
DEFAULT_LOGGER_NAME = "openagent.runtime"
LEVELS = {"DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"}


@dataclass(frozen=True, slots=True)
class RuntimeLoggingConfig:
    enabled: bool = True
    keep_records: bool = True
    jsonl: bool = False
    jsonl_dir: str = DEFAULT_JSONL_DIR
    max_records: int = DEFAULT_MAX_RECORDS
    input_preview_chars: int = DEFAULT_INPUT_PREVIEW_CHARS
    level: str = "INFO"
    python_logging: bool = False
    logger_name: str = DEFAULT_LOGGER_NAME
    include_context: bool = True


@dataclass(frozen=True, slots=True)
class RuntimeLogRecord:
    log_id: str
    timestamp_ms: int
    level: str
    message: str
    category: str
    session_id: str
    run_id: str | None = None
    trace_id: str | None = None
    span_id: str | None = None
    attributes: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


class RuntimeLogger:
    def __init__(
        self,
        *,
        session_id: str,
        session_metadata: dict[str, Any],
        config: RuntimeLoggingConfig | None = None,
        base_dir: Path | str | None = None,
        run_id: str | None = None,
        trace_id: str | None = None,
        span_getter: Any | None = None,
    ) -> None:
        self.session_id = session_id
        self.session_metadata = session_metadata
        self.config = config or RuntimeLoggingConfig()
        self.base_dir = Path(base_dir) if base_dir is not None else Path.cwd()
        self.run_id = run_id
        self.trace_id = trace_id
        self._span_getter = span_getter
        self._level_number = _level_number(self.config.level)
        self._logger = logging.getLogger(self.config.logger_name)
        if self.config.enabled:
            self._ensure_metadata_root()

    @classmethod
    def for_session(
        cls,
        *,
        session_id: str,
        session_metadata: dict[str, Any],
        options: dict[str, Any] | None,
        base_dir: Path | str | None = None,
        run_id: str | None = None,
        trace_id: str | None = None,
        span_getter: Any | None = None,
    ) -> "RuntimeLogger":
        return cls(
            session_id=session_id,
            session_metadata=session_metadata,
            config=load_runtime_logging_config(options),
            base_dir=base_dir,
            run_id=run_id,
            trace_id=trace_id,
            span_getter=span_getter,
        )

    def bind_trace(self, *, run_id: str | None, trace_id: str | None, span_getter: Any | None = None) -> None:
        self.run_id = run_id
        self.trace_id = trace_id
        if span_getter is not None:
            self._span_getter = span_getter
        if self.config.enabled:
            root = self._ensure_metadata_root()
            root["run_id"] = run_id
            root["trace_id"] = trace_id

    def debug(self, message: str, *, category: str = "runtime", attributes: dict[str, Any] | None = None) -> None:
        self.log("DEBUG", message, category=category, attributes=attributes)

    def info(self, message: str, *, category: str = "runtime", attributes: dict[str, Any] | None = None) -> None:
        self.log("INFO", message, category=category, attributes=attributes)

    def warning(self, message: str, *, category: str = "runtime", attributes: dict[str, Any] | None = None) -> None:
        self.log("WARNING", message, category=category, attributes=attributes)

    def error(self, message: str, *, category: str = "runtime", attributes: dict[str, Any] | None = None) -> None:
        self.log("ERROR", message, category=category, attributes=attributes)

    def log(
        self,
        level: str,
        message: str,
        *,
        category: str = "runtime",
        attributes: dict[str, Any] | None = None,
    ) -> RuntimeLogRecord | None:
        if not self.config.enabled:
            return None
        normalized_level = _normalize_level(level)
        if _level_number(normalized_level) < self._level_number:
            return None
        record = RuntimeLogRecord(
            log_id=new_id("log"),
            timestamp_ms=_now_ms(),
            level=normalized_level,
            message=str(message),
            category=str(category),
            session_id=self.session_id,
            run_id=self.run_id,
            trace_id=self.trace_id,
            span_id=self._current_span_id(),
            attributes=sanitize_observation_value(attributes or {}, max_chars=DEFAULT_FIELD_PREVIEW_CHARS),
        )
        self._record(record)
        return record

    def _current_span_id(self) -> str | None:
        if self._span_getter is None:
            return None
        try:
            value = self._span_getter()
        except Exception:
            return None
        return str(value) if value else None

    def _ensure_metadata_root(self) -> dict[str, Any]:
        root = self.session_metadata.get(LOGGING_METADATA_KEY)
        if not isinstance(root, dict):
            root = {}
            self.session_metadata[LOGGING_METADATA_KEY] = root
        root.setdefault("records", [])
        root.setdefault("record_count", 0)
        root["level"] = self.config.level
        root["run_id"] = self.run_id
        root["trace_id"] = self.trace_id
        root["jsonl_path"] = str(self._jsonl_path()) if self.config.jsonl else None
        return root

    def _record(self, record: RuntimeLogRecord) -> None:
        root = self._ensure_metadata_root()
        record_dict = record.to_dict()
        root["record_count"] = int(root.get("record_count") or 0) + 1
        root["last_log_at_ms"] = record.timestamp_ms
        if self.config.keep_records:
            records_raw = root.get("records")
            records = list(records_raw) if isinstance(records_raw, list) else []
            records.append(record_dict)
            root["records"] = records[-max(1, self.config.max_records):]
        if self.config.jsonl:
            self._append_jsonl(record_dict)
        if self.config.python_logging:
            self._emit_python_log(record)

    def _append_jsonl(self, record: dict[str, Any]) -> None:
        path = self._jsonl_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(record, ensure_ascii=False, sort_keys=True) + "\n")

    def _jsonl_path(self) -> Path:
        root = Path(self.config.jsonl_dir)
        if not root.is_absolute():
            root = self.base_dir / root
        run_id = self.run_id or "run_unbound"
        return root / self.session_id / f"{run_id}.jsonl"

    def _emit_python_log(self, record: RuntimeLogRecord) -> None:
        extra = {"openagent": record.to_dict()} if self.config.include_context else None
        self._logger.log(_level_number(record.level), record.message, extra=extra)


def load_runtime_logging_config(options: dict[str, Any] | None) -> RuntimeLoggingConfig:
    raw_options = options or {}
    raw = raw_options.get("logging", {})
    if raw is None:
        raw = {}
    if not isinstance(raw, dict):
        raw = {}
    return RuntimeLoggingConfig(
        enabled=_bool_option(raw.get("enabled", True)),
        keep_records=_bool_option(raw.get("keep_records", True)),
        jsonl=_bool_option(raw.get("jsonl", False)),
        jsonl_dir=str(raw.get("jsonl_dir") or DEFAULT_JSONL_DIR),
        max_records=_positive_int(raw.get("max_records"), DEFAULT_MAX_RECORDS),
        input_preview_chars=_positive_int(raw.get("input_preview_chars"), DEFAULT_INPUT_PREVIEW_CHARS),
        level=_normalize_level(raw.get("level", "INFO")),
        python_logging=_bool_option(raw.get("python_logging", False)),
        logger_name=str(raw.get("logger_name") or DEFAULT_LOGGER_NAME),
        include_context=_bool_option(raw.get("include_context", True)),
    )


def _normalize_level(value: Any) -> str:
    level = str(value or "INFO").upper()
    return level if level in LEVELS else "INFO"


def _level_number(level: str) -> int:
    return int(getattr(logging, _normalize_level(level), logging.INFO))


def _bool_option(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "on", "true", "yes"}
    return bool(value)


def _positive_int(value: Any, default: int) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        return default
    return parsed if parsed > 0 else default


def _now_ms() -> int:
    return int(time.time() * 1000)


__all__ = [
    "LOGGING_METADATA_KEY",
    "RuntimeLogger",
    "RuntimeLoggingConfig",
    "RuntimeLogRecord",
    "load_runtime_logging_config",
]
