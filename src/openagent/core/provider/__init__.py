from __future__ import annotations

from typing import Any

from .base import LanguageModel, ProviderBase
from .manager import ProviderManager

__all__ = ["LanguageModel", "ProviderBase", "ProviderManager", "create_provider"]


def __getattr__(name: str) -> Any:
    if name == "create_provider":
        from .factory import create_provider as _create_provider

        return _create_provider
    raise AttributeError(name)
