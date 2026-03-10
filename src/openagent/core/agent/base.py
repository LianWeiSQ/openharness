from __future__ import annotations

from dataclasses import dataclass

from ...adapter.agent_adapter import AgentAdapter
from ..provider.base import LanguageModel
from ..types import AgentConfig


@dataclass(slots=True)
class BaseAgent:
    config: AgentConfig
    model: LanguageModel
    system_prompt: str

    def adapter(self) -> AgentAdapter:
        return AgentAdapter(model=self.model, config=self.config)

