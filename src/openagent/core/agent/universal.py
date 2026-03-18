from __future__ import annotations

from .base import BaseAgent


class UniversalAgent(BaseAgent):
    """Primary agent for build/code tasks."""

    default_prompt_name = "build.txt"
