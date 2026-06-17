from __future__ import annotations

import json
import os
import stat
import time
from pathlib import Path
from typing import Any

DEFAULT_AUTH_FILE = "~/.config/openagent/auth.json"
SUPPORTED_PROVIDER = "openai"


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
        "api_key": api_key or existing.get("api_key"),
        "base_url": base_url or existing.get("base_url"),
        "model": model or existing.get("model"),
        "wire_api": wire_api or existing.get("wire_api"),
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
    openai = providers.get(SUPPORTED_PROVIDER) if isinstance(providers.get(SUPPORTED_PROVIDER), dict) else None
    if not openai:
        return auth_path
    set_missing_env("OPENAI_API_KEY", openai.get("api_key"))
    set_missing_env("OPENAI_BASE_URL", openai.get("base_url"))
    set_missing_env("OPENAI_MODEL", openai.get("model"))
    set_missing_env("OPENAI_WIRE_API", openai.get("wire_api"))
    return auth_path


def public_provider_record(record: dict[str, Any]) -> dict[str, Any]:
    return {
        "provider": str(record.get("provider") or SUPPORTED_PROVIDER),
        "api_key": mask_secret(str(record.get("api_key") or "")),
        "base_url": record.get("base_url"),
        "model": record.get("model"),
        "wire_api": record.get("wire_api"),
        "updated_at_ms": record.get("updated_at_ms"),
    }


def mask_secret(value: str) -> str:
    if not value:
        return ""
    if len(value) <= 8:
        return "*" * len(value)
    return value[:4] + "*" * max(4, len(value) - 8) + value[-4:]


def normalize_provider(provider: str) -> str:
    normalized = (provider or SUPPORTED_PROVIDER).strip().lower()
    if normalized != SUPPORTED_PROVIDER:
        raise ValueError(f"Unsupported provider: {provider}")
    return normalized


def set_missing_env(name: str, value: Any) -> None:
    if value and name not in os.environ:
        os.environ[name] = str(value)
