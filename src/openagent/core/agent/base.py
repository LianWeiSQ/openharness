from __future__ import annotations

from dataclasses import dataclass, field
from typing import ClassVar

from ...adapter.agent_adapter import AgentAdapter
from ...prompts import resolve_system_prompt
from ..provider.base import LanguageModel
from ..types import AgentConfig


@dataclass(slots=True)
class BaseAgent:
    default_prompt_name: ClassVar[str | None] = None

    config: AgentConfig
    model: LanguageModel
    system_prompt: str
    _uses_default_system_prompt: bool = field(init=False, default=False, repr=False)

    def __post_init__(self) -> None:
        resolved_prompt, uses_default = resolve_system_prompt(
            default_prompt_name=self.default_prompt_name,
            explicit_system_prompt=self.system_prompt,
            config_prompt=self.config.prompt,
        )
        self.system_prompt = resolved_prompt
        self._uses_default_system_prompt = uses_default

    def adapter(self) -> AgentAdapter:
        return AgentAdapter(model=self.model, config=self.config)

    @property
    def uses_default_system_prompt(self) -> bool:
        return self._uses_default_system_prompt
