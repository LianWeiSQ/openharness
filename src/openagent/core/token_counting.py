from __future__ import annotations

import json
import math
from dataclasses import dataclass
from typing import Any

from .message_materializer import MaterializedPayload
from .types import Model

DEFAULT_FALLBACK_ENCODING = "cl100k_base"


@dataclass(frozen=True, slots=True)
class TokenCountResult:
    tokens: int
    method: str
    exact: bool
    encoding_name: str | None = None
    notes: str = ""


def _load_tiktoken_module() -> Any | None:
    try:
        import tiktoken
    except ImportError:
        return None
    return tiktoken


def _resolve_encoding(module: Any, model: Model | None) -> tuple[Any, str]:
    if model is not None:
        try:
            encoding = module.encoding_for_model(model.id)
            return encoding, getattr(encoding, "name", model.id)
        except Exception:
            pass
    encoding = module.get_encoding(DEFAULT_FALLBACK_ENCODING)
    return encoding, getattr(encoding, "name", DEFAULT_FALLBACK_ENCODING)


def _serialize_payload(payload: dict[str, Any]) -> str:
    return json.dumps(payload, ensure_ascii=False, separators=(",", ":"), default=str)


def count_openai_compatible_payload(
    payload: dict[str, Any],
    *,
    model: Model | None,
    options: dict[str, Any] | None = None,
) -> TokenCountResult:
    del options
    module = _load_tiktoken_module()
    if module is None:
        raise RuntimeError("tiktoken is not installed")
    encoding, encoding_name = _resolve_encoding(module, model)
    serialized = _serialize_payload(payload)
    return TokenCountResult(
        tokens=max(1, len(encoding.encode(serialized))),
        method="tiktoken",
        exact=True,
        encoding_name=encoding_name,
    )


def estimate_payload_tokens(payload: dict[str, Any], *, bytes_per_token: int) -> TokenCountResult:
    serialized = _serialize_payload(payload)
    return TokenCountResult(
        tokens=max(1, math.ceil(len(serialized.encode("utf-8")) / bytes_per_token)),
        method="heuristic",
        exact=False,
        notes=f"bytes_per_token={bytes_per_token}",
    )


def count_materialized_payload(
    materialized: MaterializedPayload,
    *,
    model: Model | None,
    options: dict[str, Any] | None,
    counting: str,
    bytes_per_token: int,
) -> TokenCountResult:
    if counting not in {"auto", "tiktoken", "heuristic"}:
        raise ValueError(f"Unsupported counting mode: {counting}")
    if counting == "heuristic":
        return estimate_payload_tokens(materialized.payload, bytes_per_token=bytes_per_token)
    if counting == "tiktoken":
        if materialized.payload_kind != "openai_compatible":
            heuristic = estimate_payload_tokens(materialized.payload, bytes_per_token=bytes_per_token)
            return TokenCountResult(
                tokens=heuristic.tokens,
                method=heuristic.method,
                exact=heuristic.exact,
                encoding_name=heuristic.encoding_name,
                notes="payload kind is not OpenAI-compatible; fell back to heuristic",
            )
        try:
            return count_openai_compatible_payload(materialized.payload, model=model, options=options)
        except Exception as exc:  # noqa: BLE001
            heuristic = estimate_payload_tokens(materialized.payload, bytes_per_token=bytes_per_token)
            return TokenCountResult(
                tokens=heuristic.tokens,
                method=heuristic.method,
                exact=heuristic.exact,
                encoding_name=heuristic.encoding_name,
                notes=f"tiktoken unavailable: {exc}",
            )
    if materialized.payload_kind == "openai_compatible":
        try:
            return count_openai_compatible_payload(materialized.payload, model=model, options=options)
        except Exception:
            pass
    return estimate_payload_tokens(materialized.payload, bytes_per_token=bytes_per_token)


__all__ = [
    "DEFAULT_FALLBACK_ENCODING",
    "TokenCountResult",
    "count_materialized_payload",
    "count_openai_compatible_payload",
    "estimate_payload_tokens",
]
