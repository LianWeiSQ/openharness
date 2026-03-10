from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from ..id import new_id
from ..types import ChatMessage, SessionStatus


@dataclass(slots=True)
class Session:
    id: str = field(default_factory=lambda: new_id("session"))
    directory: Path = field(default_factory=lambda: Path.cwd())
    status: SessionStatus = SessionStatus.IDLE
    messages: list[ChatMessage] = field(default_factory=list)
    metadata: dict[str, Any] = field(default_factory=dict)

    def add(self, message: ChatMessage) -> None:
        self.messages.append(message)

    def fork(self, *, at: int | None = None) -> "Session":
        idx = at if at is not None else len(self.messages)
        child = Session(directory=self.directory)
        child.messages = list(self.messages[:idx])
        return child

    def revert(self, *, to: int) -> None:
        if to < 0 or to > len(self.messages):
            raise ValueError("Invalid revert index")
        del self.messages[to:]

