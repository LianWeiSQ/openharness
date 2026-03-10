from __future__ import annotations

from abc import ABC, abstractmethod
from collections.abc import AsyncIterator
from typing import Any, Protocol

from ..types import ChatMessage, Model, ToolSchema, Usage


class LanguageModel(Protocol):
    async def stream(
        self,
        *,
        system: str | None,
        messages: list[ChatMessage],
        tools: list[ToolSchema],
        temperature: float | None = None,
        max_output_tokens: int | None = None,
        options: dict[str, Any] | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        """
        Yield model events.

        Minimum required shapes:
        - {"type": "text-delta", "id": "...", "text": "..."}
        - {"type": "tool-call", "call_id": "...", "name": "...", "input": {...}}
        - {"type": "finish", "finish_reason": "...", "usage": Usage | dict}
        """


class ProviderBase(ABC):
    @abstractmethod
    async def get_language_model(self, model: Model) -> LanguageModel:
        """Return a LanguageModel instance for a given model."""

    @abstractmethod
    async def list_models(self) -> list[Model]:
        """List available models for this provider."""

    @abstractmethod
    def get_model_config(self, model: Model) -> dict[str, Any]:
        """Return provider-specific configuration for the model."""


def coerce_usage(value: Any) -> Usage:
    if isinstance(value, Usage):
        return value
    if isinstance(value, dict):
        return Usage(
            input_tokens=int(value.get("input_tokens", 0)),
            output_tokens=int(value.get("output_tokens", 0)),
            cost=float(value.get("cost", 0.0)),
        )
    return Usage()

