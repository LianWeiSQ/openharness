from __future__ import annotations

from collections.abc import AsyncIterator
from dataclasses import dataclass, field
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
    seen_tools_by_call: list[list[str]] = field(default_factory=list)
    seen_messages_by_call: list[list[Any]] = field(default_factory=list)
    seen_max_output_tokens_by_call: list[int | None] = field(default_factory=list)

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
        del system, temperature, options
        idx = self.call_index
        self.call_index += 1
        self.seen_tools_by_call.append([getattr(tool, "name", str(tool)) for tool in tools])
        self.seen_messages_by_call.append(list(messages))
        self.seen_max_output_tokens_by_call.append(max_output_tokens)
        events = self.script[idx] if idx < len(self.script) else [{"type": "finish", "finish_reason": "stop", "usage": {}}]
        for ev in events:
            yield ev
