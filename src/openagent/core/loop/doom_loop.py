from __future__ import annotations

from collections import deque

from ..types import ToolCall


class DoomLoopDetector: #智能体熔断器，3次连续调用同一工具则触发
    def __init__(self, threshold: int = 3) -> None:
        self.threshold = threshold
        self._history: deque[str] = deque(maxlen=threshold)

    def record(self, call: ToolCall) -> bool:
        self._history.append(call.key())
        if len(self._history) < self.threshold:
            return False
        first = self._history[0]
        return all(x == first for x in self._history)

