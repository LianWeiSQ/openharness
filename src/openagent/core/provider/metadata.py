from __future__ import annotations

import os
import re
from typing import Any

DEFAULT_PROVIDER = "openai"
PROVIDER_ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
DEFAULT_ENV_MAPPING = {
    "api_key": "OPENAI_API_KEY",
    "base_url": "OPENAI_BASE_URL",
    "model": "OPENAI_MODEL",
    "wire_api": "OPENAI_WIRE_API",
}
PROVIDER_ENV_PREFIXES = {
    "anthropic": "ANTHROPIC",
    "azure": "AZURE_OPENAI",
    "azure-openai": "AZURE_OPENAI",
    "cohere": "COHERE",
    "deepseek": "DEEPSEEK",
    "gemini": "GOOGLE",
    "google": "GOOGLE",
    "groq": "GROQ",
    "mistral": "MISTRAL",
    "ollama": "OLLAMA",
    "openai": "OPENAI",
    "openrouter": "OPENROUTER",
    "xai": "XAI",
}

OPENAI_COMPATIBLE_PROVIDER_IDS = frozenset(
    {
        "azure",
        "azure-openai",
        "deepseek",
        "groq",
        "mistral",
        "ollama",
        "openai",
        "openrouter",
        "xai",
    }
)

_PROVIDER_METADATA: dict[str, dict[str, Any]] = {
    "anthropic": {
        "label": "Anthropic",
        "default_model": "claude-sonnet-4-5",
        "auth_notes": "Native Anthropic Messages routing is supported with ANTHROPIC_API_KEY; well-known provider URL login remains tracked separately.",
    },
    "azure-openai": {
        "label": "Azure OpenAI",
        "default_model": "gpt-4o-mini",
        "auth_notes": "Set AZURE_OPENAI_BASE_URL to your deployment endpoint when using the OpenAI-compatible runtime.",
    },
    "cohere": {
        "label": "Cohere",
        "default_model": "command-a-03-2025",
        "auth_notes": "Native Cohere SDK routing is not implemented; use an OpenAI-compatible gateway/base URL for runtime calls.",
    },
    "deepseek": {
        "label": "DeepSeek",
        "default_base_url": "https://api.deepseek.com/v1",
        "default_model": "deepseek-chat",
    },
    "gemini": {
        "label": "Google Gemini",
        "default_model": "gemini-2.5-pro",
        "auth_notes": "Native Gemini SDK routing is not implemented; use an OpenAI-compatible gateway/base URL for runtime calls.",
    },
    "groq": {
        "label": "Groq",
        "default_base_url": "https://api.groq.com/openai/v1",
        "default_model": "llama-3.3-70b-versatile",
    },
    "mistral": {
        "label": "Mistral",
        "default_base_url": "https://api.mistral.ai/v1",
        "default_model": "mistral-large-latest",
    },
    "ollama": {
        "label": "Ollama",
        "default_base_url": "http://localhost:11434/v1",
        "default_model": "llama3.2",
        "requires_api_key": False,
    },
    "openai": {
        "label": "OpenAI",
        "default_base_url": "https://api.openai.com/v1",
        "default_model": "gpt-4o-mini",
    },
    "openrouter": {
        "label": "OpenRouter",
        "default_base_url": "https://openrouter.ai/api/v1",
        "default_model": "openai/gpt-4o-mini",
    },
    "xai": {
        "label": "xAI",
        "default_base_url": "https://api.x.ai/v1",
        "default_model": "grok-3-mini",
    },
}


def normalize_provider(provider: str | None = None) -> str:
    if provider is None:
        normalized = DEFAULT_PROVIDER
    else:
        normalized = str(provider).strip().lower()
    if not normalized or not PROVIDER_ID_RE.fullmatch(normalized):
        raise ValueError(f"Invalid provider id: {provider}")
    return normalized


def selected_provider() -> str:
    return normalize_provider(os.getenv("OPENAGENT_PROVIDER") or os.getenv("OPENAGENT_ACTIVE_PROVIDER") or DEFAULT_PROVIDER)


def default_env_mapping(provider: str) -> dict[str, str]:
    normalized = normalize_provider(provider)
    if normalized == DEFAULT_PROVIDER:
        return dict(DEFAULT_ENV_MAPPING)
    base_provider = re.split(r"[._-]+", normalized, maxsplit=1)[0]
    prefix = (
        PROVIDER_ENV_PREFIXES.get(normalized)
        or PROVIDER_ENV_PREFIXES.get(base_provider)
        or re.sub(r"[^A-Z0-9]+", "_", normalized.upper()).strip("_")
        or "OPENAGENT"
    )
    return {
        "api_key": f"{prefix}_API_KEY",
        "base_url": f"{prefix}_BASE_URL",
        "model": f"{prefix}_MODEL",
        "wire_api": f"{prefix}_WIRE_API",
    }


def normalize_env_mapping(value: Any, *, provider: str = DEFAULT_PROVIDER) -> dict[str, str]:
    env = default_env_mapping(provider)
    if isinstance(value, dict):
        for field in DEFAULT_ENV_MAPPING:
            env_name = value.get(field)
            if isinstance(env_name, str) and env_name.strip():
                env[field] = env_name.strip()
    return env


def known_provider_ids() -> list[str]:
    return sorted(_PROVIDER_METADATA)


def provider_label(provider: str) -> str:
    normalized = normalize_provider(provider)
    metadata = _PROVIDER_METADATA.get(normalized, {})
    label = metadata.get("label")
    if isinstance(label, str) and label:
        return label
    return normalized.replace("_", "-").replace(".", "-").title()


def provider_default_base_url(provider: str) -> str | None:
    metadata = _PROVIDER_METADATA.get(normalize_provider(provider), {})
    value = metadata.get("default_base_url")
    return value if isinstance(value, str) and value else None


def provider_default_model(provider: str) -> str | None:
    metadata = _PROVIDER_METADATA.get(normalize_provider(provider), {})
    value = metadata.get("default_model")
    return value if isinstance(value, str) and value else None


def provider_requires_api_key(provider: str) -> bool:
    metadata = _PROVIDER_METADATA.get(normalize_provider(provider), {})
    return bool(metadata.get("requires_api_key", True))


def provider_auth_methods(provider: str) -> list[dict[str, Any]]:
    normalized = normalize_provider(provider)
    env = default_env_mapping(normalized)
    requires_key = provider_requires_api_key(normalized)
    api_key_status = "set" if os.getenv(env["api_key"]) else ("not_required" if not requires_key else "missing")
    fields = [
        {"name": "api_key", "env": env["api_key"], "required": requires_key, "secret": True},
        {"name": "base_url", "env": env["base_url"], "required": False, "secret": False},
        {"name": "model", "env": env["model"], "required": False, "secret": False},
        {"name": "wire_api", "env": env["wire_api"], "required": False, "secret": False},
    ]
    method: dict[str, Any] = {
        "id": "api_key",
        "type": "api_key",
        "label": "API key",
        "provider": normalized,
        "provider_label": provider_label(normalized),
        "env": env,
        "fields": fields,
        "status": api_key_status,
        "available": api_key_status in {"set", "not_required"},
        "implemented": True,
    }
    default_base_url = provider_default_base_url(normalized)
    default_model = provider_default_model(normalized)
    if default_base_url:
        method["default_base_url"] = default_base_url
    if default_model:
        method["default_model"] = default_model
    notes = _PROVIDER_METADATA.get(normalized, {}).get("auth_notes")
    if isinstance(notes, str) and notes:
        method["notes"] = notes
    return [method]


def provider_auth_method_overview(provider: str) -> list[dict[str, Any]]:
    return [
        {
            "id": method["id"],
            "type": method["type"],
            "status": method["status"],
            "env_api_key": method["env"]["api_key"],
            "implemented": method["implemented"],
        }
        for method in provider_auth_methods(provider)
    ]


def provider_has_env_credential(provider: str, env: dict[str, str] | None = None) -> bool:
    mapping = env or default_env_mapping(provider)
    api_key_env = mapping.get("api_key")
    if api_key_env and os.getenv(api_key_env):
        return True
    if not provider_requires_api_key(provider):
        base_url_env = mapping.get("base_url")
        return bool(base_url_env and os.getenv(base_url_env))
    return False


def discover_env_provider_ids() -> list[str]:
    discovered = {provider for provider in known_provider_ids() if provider_has_env_credential(provider)}
    try:
        active = selected_provider()
    except ValueError:
        active = ""
    if active and provider_has_env_credential(active):
        discovered.add(active)
    return sorted(discovered)
