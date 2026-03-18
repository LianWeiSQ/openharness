from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass
class TruncatedResult:
    """Result of truncating tool output for display."""

    content: str
    truncated: bool
    original_lines: int
    original_bytes: int
    output_path: str | None = None


class Truncate:
    """Shared display truncation helper for tool output."""

    DEFAULT_MAX_LINES = 2000
    DEFAULT_MAX_BYTES = 50 * 1024

    @staticmethod
    def output(
        content: str,
        options: dict[str, Any] | None = None,
        *,
        max_lines: int | None = None,
        max_bytes: int | None = None,
    ) -> TruncatedResult:
        if options is None:
            options = {}

        resolved_max_lines = max_lines or options.get("max_lines", Truncate.DEFAULT_MAX_LINES)
        resolved_max_bytes = max_bytes or options.get("max_bytes", Truncate.DEFAULT_MAX_BYTES)

        original_lines = content.count("\n") + 1 if content else 0
        original_bytes = len(content.encode("utf-8")) if content else 0

        if original_lines <= resolved_max_lines and original_bytes <= resolved_max_bytes:
            return TruncatedResult(
                content=content,
                truncated=False,
                original_lines=original_lines,
                original_bytes=original_bytes,
            )

        lines = content.split("\n")
        if len(lines) > resolved_max_lines:
            content = "\n".join(lines[:resolved_max_lines])
            content += f"\n\n... output truncated (original {original_lines} lines, showing first {resolved_max_lines} lines)"

        current_bytes = len(content.encode("utf-8"))
        if current_bytes > resolved_max_bytes:
            encoded = content.encode("utf-8")[:resolved_max_bytes]
            content = encoded.decode("utf-8", errors="ignore")
            content += f"\n\n... output truncated (original {original_bytes} bytes)"

        return TruncatedResult(
            content=content,
            truncated=True,
            original_lines=original_lines,
            original_bytes=original_bytes,
        )

    @staticmethod
    def truncate_lines(content: str, max_lines: int = DEFAULT_MAX_LINES) -> TruncatedResult:
        return Truncate.output(content, max_lines=max_lines, max_bytes=10**9)

    @staticmethod
    def truncate_bytes(content: str, max_bytes: int = DEFAULT_MAX_BYTES) -> TruncatedResult:
        return Truncate.output(content, max_lines=10**9, max_bytes=max_bytes)
