from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass(slots=True)
class MemoryAdapter:
    data: dict[str, Any] = field(default_factory=dict)

    def read(self, key: str) -> Any:
        return self.data.get(key)

    def write(self, key: str, value: Any) -> None:
        self.data[key] = value

