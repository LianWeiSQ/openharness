from __future__ import annotations

import json
import ipaddress
import re
import shlex
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any

WELLKNOWN_PATH = "/.well-known/opencode"
ENV_NAME_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
SECRETISH_RE = re.compile(r"(api[_-]?key|bearer|password|secret|token)", re.IGNORECASE)
MAX_METADATA_BYTES = 1024 * 1024
REDIRECT_STATUSES = {301, 302, 303, 307, 308}


class WellKnownProviderError(ValueError):
    pass


@dataclass(frozen=True, slots=True)
class WellKnownProviderLogin:
    provider_url: str
    wellknown_url: str
    auth_command: list[str]
    auth_env: str
    auth_command_preview: str
    base_url: str | None
    model: str | None
    wire_api: str | None


def load_wellknown_provider_login(
    provider_url: str,
    *,
    provider: str | None = None,
    allow_insecure_localhost: bool = False,
    timeout_s: float = 10,
) -> WellKnownProviderLogin:
    normalized_url = validate_provider_url(provider_url, allow_insecure_localhost=allow_insecure_localhost)
    wellknown_url = build_wellknown_url(normalized_url)
    payload = fetch_wellknown_metadata(wellknown_url, timeout_s=timeout_s)
    command, env_name = parse_wellknown_auth(payload)
    defaults = extract_provider_defaults(
        payload,
        provider=provider,
        fallback_base_url=normalized_url,
        allow_insecure_localhost=allow_insecure_localhost,
    )
    return WellKnownProviderLogin(
        provider_url=normalized_url,
        wellknown_url=wellknown_url,
        auth_command=command,
        auth_env=env_name,
        auth_command_preview=command_preview(command),
        base_url=defaults.get("base_url"),
        model=defaults.get("model"),
        wire_api=defaults.get("wire_api"),
    )


def validate_provider_url(provider_url: str, *, allow_insecure_localhost: bool = False) -> str:
    raw_url = str(provider_url or "").strip()
    if not raw_url:
        raise WellKnownProviderError("provider URL is required")
    parsed = urllib.parse.urlparse(raw_url)
    scheme = parsed.scheme.lower()
    if scheme != "https":
        if scheme == "http" and allow_insecure_localhost and is_localhost(parsed.hostname):
            pass
        else:
            raise WellKnownProviderError("provider URL scheme must be https; http is only allowed for localhost with --allow-insecure-localhost")
    if not parsed.hostname:
        raise WellKnownProviderError("provider URL host is required")
    try:
        parsed.port
    except ValueError as error:
        raise WellKnownProviderError("provider URL port is invalid") from error
    if is_sensitive_ip_literal(parsed.hostname) and not is_localhost(parsed.hostname):
        raise WellKnownProviderError("provider URL must not use private, link-local, or reserved IP hosts")
    if is_localhost(parsed.hostname) and not allow_insecure_localhost:
        raise WellKnownProviderError("provider URL localhost access requires --allow-insecure-localhost")
    if parsed.username or parsed.password:
        raise WellKnownProviderError("provider URL must not include username or password")
    if parsed.query:
        raise WellKnownProviderError("provider URL must not include a query string")
    if parsed.fragment:
        raise WellKnownProviderError("provider URL must not include a fragment")
    path = parsed.path.rstrip("/")
    return urllib.parse.urlunparse((scheme, parsed.netloc, path, "", "", ""))


def is_localhost(hostname: str | None) -> bool:
    if hostname is None:
        return False
    return hostname.lower() in {"localhost", "127.0.0.1", "::1"}


def is_sensitive_ip_literal(hostname: str | None) -> bool:
    if hostname is None:
        return False
    try:
        address = ipaddress.ip_address(hostname)
    except ValueError:
        return False
    return bool(
        address.is_private
        or address.is_loopback
        or address.is_link_local
        or address.is_multicast
        or address.is_reserved
        or address.is_unspecified
    )


def build_wellknown_url(provider_url: str) -> str:
    return f"{provider_url.rstrip('/')}{WELLKNOWN_PATH}"


def fetch_wellknown_metadata(wellknown_url: str, *, timeout_s: float = 10) -> dict[str, Any]:
    request = urllib.request.Request(
        url=wellknown_url,
        headers={"Accept": "application/json"},
        method="GET",
    )
    try:
        with open_wellknown_request(request, timeout_s=timeout_s) as response:
            raw = response.read(MAX_METADATA_BYTES + 1)
    except urllib.error.HTTPError as error:
        close_http_error(error)
        if error.code in REDIRECT_STATUSES:
            raise WellKnownProviderError("well-known provider metadata redirects are not allowed") from error
        raise WellKnownProviderError(f"failed to fetch well-known provider metadata: HTTP {error.code}") from error
    except urllib.error.URLError as error:
        raise WellKnownProviderError(f"failed to fetch well-known provider metadata: {error.reason}") from error
    if len(raw) > MAX_METADATA_BYTES:
        raise WellKnownProviderError("well-known provider metadata is too large")
    try:
        payload = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise WellKnownProviderError("well-known provider metadata must be valid UTF-8 JSON") from error
    if not isinstance(payload, dict):
        raise WellKnownProviderError("well-known provider metadata must be a JSON object")
    return payload


def close_http_error(error: urllib.error.HTTPError) -> None:
    try:
        error.close()
    except Exception:
        return


def open_wellknown_request(request: urllib.request.Request, *, timeout_s: float) -> Any:
    opener = urllib.request.build_opener(NoRedirectHandler)
    return opener.open(request, timeout=timeout_s)  # noqa: S310 - request URL is validated before this helper is called.


class NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(
        self,
        req: urllib.request.Request,
        fp: Any,
        code: int,
        msg: str,
        headers: Any,
        newurl: str,
    ) -> None:
        return None


def parse_wellknown_auth(payload: dict[str, Any]) -> tuple[list[str], str]:
    auth = payload.get("auth")
    if not isinstance(auth, dict):
        raise WellKnownProviderError("well-known provider metadata must include an auth object")
    command = auth.get("command")
    if not isinstance(command, list) or not command or any(not isinstance(part, str) or not part for part in command):
        raise WellKnownProviderError("well-known auth.command must be a non-empty argv list of strings")
    env_name = auth.get("env")
    if not isinstance(env_name, str) or not ENV_NAME_RE.fullmatch(env_name):
        raise WellKnownProviderError("well-known auth.env must be a valid environment variable name")
    return list(command), env_name


def extract_provider_defaults(
    payload: dict[str, Any],
    *,
    provider: str | None,
    fallback_base_url: str,
    allow_insecure_localhost: bool,
) -> dict[str, str | None]:
    defaults: dict[str, str | None] = {
        "base_url": fallback_base_url,
        "model": None,
        "wire_api": None,
    }
    provider_config = provider_config_from_payload(payload, provider)
    if not provider_config:
        return defaults
    options = provider_config.get("options") if isinstance(provider_config.get("options"), dict) else {}
    base_url = first_string(
        options.get("baseURL"),
        options.get("base_url"),
        provider_config.get("baseURL"),
        provider_config.get("base_url"),
    )
    if base_url:
        defaults["base_url"] = validate_provider_url(base_url, allow_insecure_localhost=allow_insecure_localhost)
    model = first_string(provider_config.get("model"), provider_config.get("default_model"), provider_config.get("defaultModel"))
    models = provider_config.get("models")
    if not model and isinstance(models, dict) and models:
        for key in models:
            if isinstance(key, str) and key:
                model = key
                break
    if model:
        defaults["model"] = model
    wire_api = first_string(provider_config.get("wire_api"), provider_config.get("wireAPI"), options.get("wire_api"), options.get("wireAPI"))
    if wire_api in {"chat", "responses"}:
        defaults["wire_api"] = wire_api
    return defaults


def provider_config_from_payload(payload: dict[str, Any], provider: str | None) -> dict[str, Any] | None:
    config = payload.get("config")
    if not isinstance(config, dict):
        return None
    providers = config.get("provider")
    if not isinstance(providers, dict):
        return None
    if provider and isinstance(providers.get(provider), dict):
        return providers[provider]
    for value in providers.values():
        if isinstance(value, dict):
            return value
    return None


def first_string(*values: Any) -> str | None:
    for value in values:
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


def command_preview(command: list[str]) -> str:
    return shlex.join(redact_command_preview_args(command))


def redact_command_preview_args(command: list[str]) -> list[str]:
    redacted: list[str] = []
    redact_next = False
    header_next = False
    for part in command:
        if redact_next or header_next or should_redact_command_part(part):
            redacted.append("[redacted]")
            redact_next = False
            header_next = False
        else:
            redacted.append(part)
        lowered = part.lower()
        if lowered in {"-h", "--header", "--headers"}:
            header_next = True
        if lowered in {"--api-key", "--apikey", "--token", "--secret", "--password", "--bearer", "--credential", "--private-key"}:
            redact_next = True
    return redacted


def should_redact_command_part(part: str) -> bool:
    lowered = part.lower()
    if SECRETISH_RE.search(part) or any(marker in lowered for marker in ("credential", "authorization", "cookie", "private-key", "private_key")):
        return True
    if re.search(r"(?i)\b(bearer|basic)\s+[a-z0-9._~+/=-]{8,}", part):
        return True
    if re.search(r"(?i)\b(api[_-]?key|token|secret|password|credential|private[_-]?key|authorization|cookie)=.+", part):
        return True
    if re.fullmatch(r"[A-Za-z0-9._~+/=-]{24,}", part):
        return True
    return False
