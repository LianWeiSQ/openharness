from __future__ import annotations

import unittest

from openagent.core.agent.explore import ExploreAgent
from openagent.core.agent.plan import PlanAgent
from openagent.core.agent.universal import UniversalAgent
from openagent.core.types import AgentConfig
from openagent.prompts import load_prompt

from _mock_model import ScriptedLanguageModel


class AgentPromptTests(unittest.TestCase):
    def _model(self) -> ScriptedLanguageModel:
        return ScriptedLanguageModel(script=[])

    def test_universal_agent_loads_default_prompt_and_appends_config_prompt(self) -> None:
        cfg = AgentConfig(name="u", prompt="Extra prompt instructions.")
        agent = UniversalAgent(config=cfg, model=self._model(), system_prompt="")

        self.assertTrue(agent.uses_default_system_prompt)
        self.assertEqual(agent.system_prompt, f"{load_prompt('build.txt')}\n\nExtra prompt instructions.")

    def test_explicit_system_prompt_overrides_default_prompt_and_config_prompt(self) -> None:
        cfg = AgentConfig(name="u", prompt="Should be ignored.")
        agent = UniversalAgent(config=cfg, model=self._model(), system_prompt="Custom system prompt.")

        self.assertFalse(agent.uses_default_system_prompt)
        self.assertEqual(agent.system_prompt, "Custom system prompt.")

    def test_plan_and_explore_agents_load_their_default_prompts(self) -> None:
        plan_agent = PlanAgent(config=AgentConfig(name="p"), model=self._model(), system_prompt="")
        explore_agent = ExploreAgent(config=AgentConfig(name="e"), model=self._model(), system_prompt="")

        self.assertTrue(plan_agent.uses_default_system_prompt)
        self.assertEqual(plan_agent.system_prompt, load_prompt("plan.txt"))
        self.assertTrue(explore_agent.uses_default_system_prompt)
        self.assertEqual(explore_agent.system_prompt, load_prompt("explore.txt"))
