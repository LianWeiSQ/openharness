from __future__ import annotations

from functools import lru_cache
from pathlib import Path

PROMPT_DIR = Path(__file__).resolve().parent


@lru_cache(maxsize=None)
def load_prompt(prompt_name: str) -> str:
    path = PROMPT_DIR / prompt_name
    return path.read_text(encoding="utf-8").strip()


def resolve_system_prompt(
    *,
    default_prompt_name: str | None,
    explicit_system_prompt: str,
    config_prompt: str | None,
) -> tuple[str, bool]:
    if explicit_system_prompt:
        return explicit_system_prompt, False

    base_prompt = load_prompt(default_prompt_name) if default_prompt_name else ""
    if config_prompt:
        if base_prompt:
            return f"{base_prompt}\n\n{config_prompt.strip()}", True
        return config_prompt.strip(), False
    return base_prompt, bool(base_prompt)


__all__ = ["load_prompt", "resolve_system_prompt"]
