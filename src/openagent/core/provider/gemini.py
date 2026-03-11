from __future__ import annotations

from typing import Any

from ..types import Model
from .base import LanguageModel, ProviderBase

# 待实现
class GeminiProvider(ProviderBase):
    async def get_language_model(self, model: Model) -> LanguageModel:  # pragma: no cover
        raise NotImplementedError("GeminiProvider is a stub; wire your Google client here.")

    async def list_models(self) -> list[Model]:
        return []

    def get_model_config(self, model: Model) -> dict[str, Any]:
        return {}

