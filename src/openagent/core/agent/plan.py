from __future__ import annotations

from .base import BaseAgent


class PlanAgent(BaseAgent):
    """Primary agent for planning and architecture."""

    default_prompt_name = "plan.txt"
