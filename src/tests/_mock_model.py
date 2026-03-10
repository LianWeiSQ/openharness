from __future__ import annotations

from collections.abc import AsyncIterator
from dataclasses import dataclass
from typing import Any

from openagent.core.provider.base import LanguageModel


@dataclass(slots=True)
class ScriptedLanguageModel(LanguageModel):
    """
    A deterministic LanguageModel for tests.

    Script is a list of per-call event lists.
    """

    script: list[list[dict[str, Any]]]
    call_index: int = 0

    async def stream(
        self,
        *,
        system: str | None,
        messages: list[Any],
        tools: list[Any],
        temperature: float | None = None,
        max_output_tokens: int | None = None,
        options: dict[str, Any] | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        idx = self.call_index
        self.call_index += 1
        events = self.script[idx] if idx < len(self.script) else [{"type": "finish", "finish_reason": "stop", "usage": {}}]
        for ev in events:
            yield ev

