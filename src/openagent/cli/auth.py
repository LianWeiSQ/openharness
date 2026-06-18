from __future__ import annotations

import json
import os
import re
import stat
import time
from pathlib import Path
from typing import Any

DEFAULT_AUTH_FILE = "~/.config/openagent/auth.json"
DEFAULT_PROVIDER = "openai"
SUPPORTED_PROVIDER = DEFAULT_PROVIDER
PROVIDER_ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
DEFAULT_ENV_MAPPING = {
    "api_key": "OPENAI_API_KEY",
    "base_url": "OPENAI_BASE_URL",
    "model": "OPENAI_MODEL",
    "wire_api": "OPENAI_WIRE_API",
}


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
        "env": normalize_env_mapping(existing.get("env")),
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
    return [public_provider_record(record) for _, record in sorted(providers.items()) if isinstance(record, dict)]


def load_auth_env(path: str | None = None) -> Path | None:
    auth_path = resolve_auth_file(path)
    if not auth_path.exists():
        return None
    payload = load_auth_file(auth_path)
    providers = payload.get("providers") if isinstance(payload.get("providers"), dict) else {}
    provider = selected_provider()
    record = providers.get(provider) if isinstance(providers.get(provider), dict) else None
    if not record:
        return auth_path
    set_missing_env("OPENAGENT_PROVIDER", provider)
    set_missing_env("OPENAGENT_ACTIVE_PROVIDER", provider)
    env = normalize_env_mapping(record.get("env"))
    set_missing_env("OPENAI_API_KEY", record.get("api_key"))
    set_missing_env("OPENAI_BASE_URL", record.get("base_url"))
    set_missing_env("OPENAI_MODEL", record.get("model"))
    set_missing_env("OPENAI_WIRE_API", record.get("wire_api"))
    for field, env_name in env.items():
        value = record.get(field)
        if env_name in DEFAULT_ENV_MAPPING.values():
            continue
        set_missing_env(env_name, value)
    return auth_path


def public_provider_record(record: dict[str, Any]) -> dict[str, Any]:
    env = normalize_env_mapping(record.get("env"))
    return {
        "provider": normalize_provider(str(record.get("provider") or DEFAULT_PROVIDER)),
        "type": str(record.get("type") or "api"),
        "api_key": mask_secret(str(record.get("api_key") or "")),
        "has_api_key": bool(record.get("api_key")),
        "base_url": record.get("base_url"),
        "model": record.get("model"),
        "wire_api": record.get("wire_api"),
        "env": env,
        "env_status": env_status(env),
        "updated_at_ms": record.get("updated_at_ms"),
    }


def mask_secret(value: str) -> str:
    if not value:
        return ""
    if len(value) <= 8:
        return "*" * len(value)
    return value[:4] + "*" * max(4, len(value) - 8) + value[-4:]


def normalize_provider(provider: str) -> str:
    normalized = (provider or DEFAULT_PROVIDER).strip().lower()
    if not normalized or not PROVIDER_ID_RE.fullmatch(normalized):
        raise ValueError(f"Invalid provider id: {provider}")
    return normalized


def selected_provider() -> str:
    return normalize_provider(os.getenv("OPENAGENT_PROVIDER") or os.getenv("OPENAGENT_ACTIVE_PROVIDER") or DEFAULT_PROVIDER)


def normalize_env_mapping(value: Any) -> dict[str, str]:
    env = dict(DEFAULT_ENV_MAPPING)
    if isinstance(value, dict):
        for field in DEFAULT_ENV_MAPPING:
            env_name = value.get(field)
            if isinstance(env_name, str) and env_name.strip():
                env[field] = env_name.strip()
    return env


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
