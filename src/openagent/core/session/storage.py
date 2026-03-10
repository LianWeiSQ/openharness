from __future__ import annotations

import json
from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from typing import Any


class StorageBase(ABC):
    @abstractmethod
    def read(self, key: str) -> dict[str, Any] | None:
        raise NotImplementedError

    @abstractmethod
    def write(self, key: str, value: dict[str, Any]) -> None:
        raise NotImplementedError


class InMemoryStorage(StorageBase):
    def __init__(self) -> None:
        self._data: dict[str, dict[str, Any]] = {}

    def read(self, key: str) -> dict[str, Any] | None:
        return self._data.get(key)

    def write(self, key: str, value: dict[str, Any]) -> None:
        self._data[key] = value


@dataclass(slots=True)
class JsonFileStorage(StorageBase):
    root: Path

    def read(self, key: str) -> dict[str, Any] | None:
        path = self.root / f"{key}.json"
        if not path.exists():
            return None
        return json.loads(path.read_text(encoding="utf-8"))

    def write(self, key: str, value: dict[str, Any]) -> None:
        self.root.mkdir(parents=True, exist_ok=True)
        path = self.root / f"{key}.json"
        path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")

