from __future__ import annotations

from typing import Any

from ..types import Model
from .base import LanguageModel, ProviderBase


class AnthropicProvider(ProviderBase):
    async def get_language_model(self, model: Model) -> LanguageModel:  # pragma: no cover
        raise NotImplementedError("AnthropicProvider is a stub; wire your Anthropic client here.")

    async def list_models(self) -> list[Model]:
        return []

    def get_model_config(self, model: Model) -> dict[str, Any]:
        return {}

