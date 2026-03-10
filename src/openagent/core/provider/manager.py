from __future__ import annotations

from dataclasses import dataclass

from ..types import Model
from .base import ProviderBase


@dataclass(frozen=True, slots=True)
class ProviderInfo:
    id: str
    name: str


class ProviderManager:
    def __init__(self) -> None:
        self._providers: dict[str, ProviderBase] = {}

    def register_provider(self, provider_id: str, provider: ProviderBase) -> None:
        self._providers[provider_id] = provider

    def get_provider(self, provider_id: str) -> ProviderBase:
        if provider_id not in self._providers:
            raise KeyError(f"Unknown provider: {provider_id}")
        return self._providers[provider_id]

    async def list_models(self) -> list[Model]:
        models: list[Model] = []
        for provider in self._providers.values():
            models.extend(await provider.list_models())
        return models

    async def default_model(self) -> Model:
        models = await self.list_models()
        if not models:
            raise RuntimeError("No models available in ProviderManager")
        return models[0]

