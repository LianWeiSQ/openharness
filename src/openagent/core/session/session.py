from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from ..id import new_id
from ..types import ChatMessage, SessionStatus
from .todo import TodoItem


@dataclass(slots=True)
class Session:
    id: str = field(default_factory=lambda: new_id("session"))
    directory: Path = field(default_factory=lambda: Path.cwd())
    status: SessionStatus = SessionStatus.IDLE
    messages: list[ChatMessage] = field(default_factory=list)
    todos: list[TodoItem] = field(default_factory=list)
    metadata: dict[str, Any] = field(default_factory=dict)
    _store: Any | None = field(default=None, repr=False, compare=False)
    _store_run_id: str | None = field(default=None, repr=False, compare=False)

    def add(self, message: ChatMessage) -> None:
        self.messages.append(message)
        if self._store is not None and self._store_run_id:
            self._store.append_message(self, message, run_id=self._store_run_id, index=len(self.messages) - 1)

    def set_todos(self, todos: list[TodoItem]) -> None:
        self.todos = list(todos)

    def bind_store(self, store: Any, *, run_id: str) -> None:
        self._store = store
        self._store_run_id = run_id

    def unbind_store(self) -> None:
        self._store = None
        self._store_run_id = None

    def remember_file_read(self, path: str | Path) -> None:
        normalized = self._normalize_file_key(path)
        files = set(self.metadata.get("_read_files", []))
        files.add(normalized)
        self.metadata["_read_files"] = sorted(files)

    def has_read_file(self, path: str | Path) -> bool:
        normalized = self._normalize_file_key(path)
        return normalized in set(self.metadata.get("_read_files", []))

    def fork(self, *, at: int | None = None) -> "Session":
        idx = at if at is not None else len(self.messages)
        child = Session(directory=self.directory)
        child.messages = list(self.messages[:idx])
        child.todos = list(self.todos)
        child.metadata = dict(self.metadata)
        return child

    def revert(self, *, to: int) -> None:
        if to < 0 or to > len(self.messages):
            raise ValueError("Invalid revert index")
        del self.messages[to:]

    @staticmethod
    def _normalize_file_key(path: str | Path) -> str:
        if isinstance(path, str) and "://" in path:
            return path
        return str(Path(path).resolve())
