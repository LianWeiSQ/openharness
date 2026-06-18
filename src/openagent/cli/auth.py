from __future__ import annotations

import json
import os
import stat
import time
from pathlib import Path
from typing import Any

from openagent.core.provider.metadata import (
    DEFAULT_ENV_MAPPING,
    DEFAULT_PROVIDER,
    default_env_mapping,
    discover_env_provider_ids,
    normalize_env_mapping,
    normalize_provider,
    provider_auth_method_overview,
    provider_default_base_url,
    provider_default_model,
    provider_has_env_credential,
    selected_provider,
)

DEFAULT_AUTH_FILE = "~/.config/openagent/auth.json"


def resolve_auth_file(path: str | None = None) -> Path:
    return Path(path or os.getenv("OPENAGENT_AUTH_FILE") or DEFAULT_AUTH_FILE).expanduser()


def load_auth_file(path: str | Path | None = None) -> dict[str, Any]:
    auth_path = resolve_auth_file(str(path) if path is not None else None)
    if not auth_path.exists():
        return {"schema_version": "openagent.auth.v1", "providers": {}}
    payload = json.loads(auth_path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        return {"schema_version": "openagent.auth.v1", "providers": {}}
    providers = payload.get("providers")
    if not isinstance(providers, dict):
        payload["providers"] = {}
    return payload


def save_auth_file(payload: dict[str, Any], path: str | Path | None = None) -> Path:
    auth_path = resolve_auth_file(str(path) if path is not None else None)
    auth_path.parent.mkdir(parents=True, exist_ok=True)
    data = json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    tmp = auth_path.with_suffix(auth_path.suffix + ".tmp")
    tmp.write_text(data, encoding="utf-8")
    os.chmod(tmp, stat.S_IRUSR | stat.S_IWUSR)
    tmp.replace(auth_path)
    os.chmod(auth_path, stat.S_IRUSR | stat.S_IWUSR)
    return auth_path


def login_provider(
    *,
    provider: str,
    credential_type: str | None = None,
    api_key: str | None,
    base_url: str | None,
    model: str | None,
    wire_api: str | None,
    path: str | Path | None = None,
) -> dict[str, Any]:
    normalized = normalize_provider(provider)
    payload = load_auth_file(path)
    providers = payload.setdefault("providers", {})
    existing = providers.get(normalized) if isinstance(providers.get(normalized), dict) else {}
    if not api_key and not existing.get("api_key"):
        raise ValueError("api key is required for first login")
    record = {
        "provider": normalized,
        "type": credential_type or existing.get("type") or "api",
        "api_key": api_key or existing.get("api_key"),
        "base_url": base_url or existing.get("base_url"),
        "model": model or existing.get("model"),
        "wire_api": wire_api or existing.get("wire_api"),
        "env": normalize_env_mapping(existing.get("env"), provider=normalized),
        "updated_at_ms": int(time.time() * 1000),
    }
    providers[normalized] = {key: value for key, value in record.items() if value is not None}
    auth_path = save_auth_file(payload, path)
    return {"provider": normalized, "auth_file": str(auth_path), "record": public_provider_record(providers[normalized])}


def logout_provider(*, provider: str, path: str | Path | None = None) -> dict[str, Any]:
    normalized = normalize_provider(provider)
    payload = load_auth_file(path)
    providers = payload.setdefault("providers", {})
    existed = normalized in providers
    providers.pop(normalized, None)
    auth_path = save_auth_file(payload, path)
    return {"provider": normalized, "auth_file": str(auth_path), "removed": existed}


def list_providers(path: str | Path | None = None) -> list[dict[str, Any]]:
    payload = load_auth_file(path)
    providers = payload.get("providers") if isinstance(payload.get("providers"), dict) else {}
    records_by_provider: dict[str, dict[str, Any]] = {}
    for provider, record in sorted(providers.items()):
        if isinstance(record, dict):
            public_record = public_provider_record(
                {
                    **record,
                    "provider": record.get("provider") or provider,
                    "source": "auth_file",
                }
            )
            records_by_provider[public_record["provider"]] = public_record
    claimed_api_key_envs = {
        row_env.get("api_key")
        for record in records_by_provider.values()
        if isinstance(record.get("env"), dict)
        for row_env in [record["env"]]
    }
    for provider in discover_env_provider_ids():
        if provider in records_by_provider:
            continue
        env = default_env_mapping(provider)
        if env["api_key"] in claimed_api_key_envs:
            continue
        public_record = public_provider_record(
            {
                "provider": provider,
                "type": "env",
                "source": "env",
                "base_url": os.getenv(env["base_url"]) or provider_default_base_url(provider),
                "model": os.getenv(env["model"]) or provider_default_model(provider),
                "wire_api": os.getenv(env["wire_api"]),
                "env": env,
            }
        )
        records_by_provider[provider] = public_record
    return [records_by_provider[provider] for provider in sorted(records_by_provider)]


def load_auth_env(path: str | None = None) -> Path | None:
    auth_path = resolve_auth_file(path)
    payload = load_auth_file(auth_path) if auth_path.exists() else {"schema_version": "openagent.auth.v1", "providers": {}}
    providers = payload.get("providers") if isinstance(payload.get("providers"), dict) else {}
    try:
        provider = selected_provider()
    except ValueError:
        return auth_path if auth_path.exists() else None
    record = providers.get(provider) if isinstance(providers.get(provider), dict) else None
    if not record and not provider_has_env_credential(provider):
        return auth_path if auth_path.exists() else None
    set_missing_env("OPENAGENT_PROVIDER", provider)
    set_missing_env("OPENAGENT_ACTIVE_PROVIDER", provider)
    env = normalize_env_mapping(record.get("env"), provider=provider) if record else default_env_mapping(provider)
    set_missing_env("OPENAI_API_KEY", provider_value("api_key", record=record, env=env, provider=provider))
    set_missing_env("OPENAI_BASE_URL", provider_value("base_url", record=record, env=env, provider=provider))
    set_missing_env("OPENAI_MODEL", provider_value("model", record=record, env=env, provider=provider))
    set_missing_env("OPENAI_WIRE_API", provider_value("wire_api", record=record, env=env, provider=provider))
    for field, env_name in env.items():
        if env_name in DEFAULT_ENV_MAPPING.values():
            continue
        value = provider_value(field, record=record, env=env, provider=provider)
        set_missing_env(env_name, value)
    return auth_path if auth_path.exists() else None


def public_provider_record(record: dict[str, Any]) -> dict[str, Any]:
    provider = normalize_provider(str(record.get("provider") or DEFAULT_PROVIDER))
    env = normalize_env_mapping(record.get("env"), provider=provider)
    source = str(record.get("source") or "auth_file")
    stored_api_key = record.get("api_key")
    env_api_key = os.getenv(env["api_key"])
    api_key = stored_api_key if stored_api_key else (env_api_key if source != "env" else "")
    auth_methods = provider_auth_method_overview(provider)
    return {
        "provider": provider,
        "type": str(record.get("type") or "api"),
        "source": source,
        "api_key": mask_secret(str(api_key or "")),
        "has_api_key": bool(stored_api_key or env_api_key),
        "base_url": record.get("base_url") or os.getenv(env["base_url"]) or provider_default_base_url(provider),
        "model": record.get("model") or os.getenv(env["model"]) or provider_default_model(provider),
        "wire_api": record.get("wire_api") or os.getenv(env["wire_api"]),
        "env": env,
        "env_status": env_status(env),
        "auth_methods": auth_methods,
        "methods": [method["id"] for method in auth_methods],
        "updated_at_ms": record.get("updated_at_ms"),
    }


def mask_secret(value: str) -> str:
    if not value:
        return ""
    if len(value) <= 8:
        return "*" * len(value)
    return value[:4] + "*" * max(4, len(value) - 8) + value[-4:]


def env_status(env: dict[str, str]) -> dict[str, dict[str, str]]:
    return {
        field: {
            "name": env_name,
            "status": "set" if os.getenv(env_name) else "missing",
        }
        for field, env_name in env.items()
    }


def set_missing_env(name: str, value: Any) -> None:
    if value and name not in os.environ:
        os.environ[name] = str(value)


def provider_value(field: str, *, record: dict[str, Any] | None, env: dict[str, str], provider: str) -> Any:
    if record and record.get(field):
        return record.get(field)
    env_name = env.get(field)
    if env_name and os.getenv(env_name):
        return os.getenv(env_name)
    if field == "base_url":
        return provider_default_base_url(provider)
    if field == "model":
        return provider_default_model(provider)
    return None
