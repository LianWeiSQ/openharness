from __future__ import annotations

import os

from .anthropic import AnthropicProvider
from .base import ProviderBase
from .metadata import default_env_mapping, normalize_provider, provider_default_base_url, selected_provider
from .openai import OpenAIProvider


def create_provider(provider: str | None = None) -> ProviderBase:
    if provider is None:
        try:
            normalized = selected_provider()
        except ValueError:
            return OpenAIProvider()
    else:
        normalized = normalize_provider(provider)
    if normalized == "anthropic":
        return AnthropicProvider()
    if provider is None:
        return OpenAIProvider()
    return _create_openai_compatible_provider(normalized)


def _create_openai_compatible_provider(provider: str) -> OpenAIProvider:
    env = default_env_mapping(provider)
    instance = OpenAIProvider(
        api_key=os.getenv("OPENAI_API_KEY") or os.getenv(env["api_key"]) or "",
        base_url=os.getenv("OPENAI_BASE_URL") or os.getenv(env["base_url"]) or provider_default_base_url(provider),
        host_header=os.getenv("OPENAI_HOST_HEADER") or None,
        wire_api=os.getenv("OPENAI_WIRE_API") or os.getenv(env["wire_api"]) or None,
    )
    instance.provider_id = provider
    return instance
