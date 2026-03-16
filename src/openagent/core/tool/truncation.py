"""
Output Truncation - 输出截断工具。

用于截断过长的工具输出，避免超出 LLM 上下文限制。
参考 OpenCode 的 Truncate.output 设计。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass
class TruncatedResult:
    """截断结果。"""

    content: str
    """截断后的内容"""

    truncated: bool
    """是否被截断"""

    original_lines: int
    """原始行数"""

    original_bytes: int
    """原始字节数"""

    output_path: str | None = None
    """完整输出保存的文件路径（如果截断）"""


class Truncate:
    """
    输出截断工具类。

    提供静态方法用于截断过长的工具输出。
    """

    # 默认配置
    DEFAULT_MAX_LINES = 2000
    DEFAULT_MAX_BYTES = 512 * 1024  # 512 KB

    @staticmethod
    def output(
        content: str,
        options: dict[str, Any] | None = None,
        *,
        max_lines: int | None = None,
        max_bytes: int | None = None,
    ) -> TruncatedResult:
        """
        截断输出内容。

        Args:
            content: 原始输出内容
            options: 配置选项（可覆盖默认值）
            max_lines: 最大行数
            max_bytes: 最大字节数

        Returns:
            TruncatedResult 包含截断后的内容和元数据
        """
        if options is None:
            options = {}

        # 确定截断参数
        _max_lines = max_lines or options.get("max_lines", Truncate.DEFAULT_MAX_LINES)
        _max_bytes = max_bytes or options.get("max_bytes", Truncate.DEFAULT_MAX_BYTES)

        # 计算原始尺寸
        original_lines = content.count("\n") + 1 if content else 0
        original_bytes = len(content.encode("utf-8")) if content else 0

        # 检查是否需要截断
        if original_lines <= _max_lines and original_bytes <= _max_bytes:
            return TruncatedResult(
                content=content,
                truncated=False,
                original_lines=original_lines,
                original_bytes=original_bytes,
            )

        # 执行截断
        lines = content.split("\n")

        # 先按行数截断
        if len(lines) > _max_lines:
            truncated_lines = lines[:_max_lines]
            content = "\n".join(truncated_lines)
            content += f"\n\n... 输出已截断（原始 {original_lines} 行，显示前 {_max_lines} 行）"

        # 再按字节数截断
        current_bytes = len(content.encode("utf-8"))
        if current_bytes > _max_bytes:
            # 按字节截断，确保 UTF-8 完整性
            encoded = content.encode("utf-8")
            truncated_bytes = encoded[:_max_bytes]
            # 尝试解码，失败则回退到最后一个有效 UTF-8 边界
            try:
                content = truncated_bytes.decode("utf-8")
            except UnicodeDecodeError:
                # 回退到最后一个有效边界
                for i in range(min(4, len(truncated_bytes)), 0, -1):
                    try:
                        content = truncated_bytes[:-i].decode("utf-8")
                        break
                    except UnicodeDecodeError:
                        continue
            content += f"\n\n... 输出已截断（原始 {original_bytes} 字节）"

        return TruncatedResult(
            content=content,
            truncated=True,
            original_lines=original_lines,
            original_bytes=original_bytes,
        )

    @staticmethod
    def truncate_lines(content: str, max_lines: int = DEFAULT_MAX_LINES) -> TruncatedResult:
        """仅按行数截断。"""
        return Truncate.output(content, max_lines=max_lines, max_bytes=10**9)

    @staticmethod
    def truncate_bytes(content: str, max_bytes: int = DEFAULT_MAX_BYTES) -> TruncatedResult:
        """仅按字节数截断。"""
        return Truncate.output(content, max_lines=10**9, max_bytes=max_bytes)
