from __future__ import annotations

import unittest

from openagent.core.context_budget import ContextBudgetConfigError, check_context_budget
from openagent.core.types import ChatMessage, Model, ToolSchema


def _make_model(*, context_window: int = 256, max_output: int = 64) -> Model:
    return Model(
        id="test-model",
        provider_id="test",
        name="Test Model",
        context_window=context_window,
        max_output=max_output,
    )


class ContextBudgetTests(unittest.TestCase):
    def test_short_context_fits_budget(self) -> None:
        result = check_context_budget(
            system="You are helpful.",
            messages=[ChatMessage(role="user", content="hello")],
            tools=[],
            model=_make_model(),
            options={},
        )

        self.assertIsNotNone(result)
        self.assertFalse(result.overflowed)
        self.assertGreater(result.estimated_input_tokens, 0)

    def test_long_context_overflows_budget(self) -> None:
        result = check_context_budget(
            system="You are helpful.",
            messages=[ChatMessage(role="user", content="x" * 800)],
            tools=[],
            model=_make_model(context_window=96, max_output=24),
            options={},
        )

        self.assertIsNotNone(result)
        self.assertTrue(result.overflowed)
        self.assertGreater(result.estimated_input_tokens, result.input_limit_tokens)

    def test_tool_schemas_are_counted(self) -> None:
        base = check_context_budget(
            system="You are helpful.",
            messages=[ChatMessage(role="user", content="hello")],
            tools=[],
            model=_make_model(),
            options={},
        )
        with_tool = check_context_budget(
            system="You are helpful.",
            messages=[ChatMessage(role="user", content="hello")],
            tools=[
                ToolSchema(
                    name="large_tool",
                    description="A" * 600,
                    schema={"type": "object", "properties": {"query": {"type": "string", "description": "B" * 600}}},
                )
            ],
            model=_make_model(),
            options={},
        )

        self.assertIsNotNone(base)
        self.assertIsNotNone(with_tool)
        self.assertGreater(with_tool.estimated_input_tokens, base.estimated_input_tokens)

    def test_unsupported_strategy_raises_clear_error(self) -> None:
        with self.assertRaises(ContextBudgetConfigError) as ctx:
            check_context_budget(
                system="You are helpful.",
                messages=[ChatMessage(role="user", content="hello")],
                tools=[],
                model=_make_model(),
                options={"context_budget": {"strategy": "compact"}},
            )

        self.assertIn("compact", str(ctx.exception))
        self.assertIn("Supported strategies: error", str(ctx.exception))
