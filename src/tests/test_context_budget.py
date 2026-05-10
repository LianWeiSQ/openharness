from __future__ import annotations

import unittest
from unittest.mock import patch

from openagent.core.context_budget import ContextBudgetConfigError, check_context_budget, load_context_budget_options
from openagent.core.message_materializer import materialize_openai_compatible_payload
from openagent.core.types import ChatMessage, Model, ToolSchema


class _FakeEncoding:
    name = "fake-encoding"

    def encode(self, text: str) -> list[int]:
        return list(text.encode("utf-8"))


class _FakeTiktoken:
    def encoding_for_model(self, model_name: str) -> _FakeEncoding:
        return _FakeEncoding()

    def get_encoding(self, name: str) -> _FakeEncoding:
        return _FakeEncoding()


def _make_model(*, context_window: int = 256, max_output: int = 64, provider_id: str = "test") -> Model:
    return Model(
        id="test-model",
        provider_id=provider_id,
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
        self.assertEqual(result.counting_method, "heuristic")
        self.assertEqual(result.fallback_stage, "initial")

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

    def test_auto_strategy_is_default(self) -> None:
        result = check_context_budget(
            system="You are helpful.",
            messages=[ChatMessage(role="user", content="hello")],
            tools=[],
            model=_make_model(),
            options={},
        )

        self.assertIsNotNone(result)
        self.assertFalse(result.overflowed)
        self.assertEqual(result.fallback_stage, "initial")

    def test_compact_strategy_is_supported(self) -> None:
        result = check_context_budget(
            system="You are helpful.",
            messages=[ChatMessage(role="user", content="hello")],
            tools=[],
            model=_make_model(),
            options={"context_budget": {"strategy": "compact"}},
        )

        self.assertIsNotNone(result)
        self.assertFalse(result.overflowed)

    def test_invalid_strategy_raises_clear_error(self) -> None:
        with self.assertRaises(ContextBudgetConfigError) as ctx:
            check_context_budget(
                system="You are helpful.",
                messages=[ChatMessage(role="user", content="hello")],
                tools=[],
                model=_make_model(),
                options={"context_budget": {"strategy": "trim"}},
            )

        self.assertIn("trim", str(ctx.exception))
        self.assertIn("Supported strategies: auto, error, compact", str(ctx.exception))

    def test_tool_message_diagnostics_are_populated(self) -> None:
        result = check_context_budget(
            system="You are helpful.",
            messages=[
                ChatMessage(role="user", content="find matches"),
                ChatMessage(role="tool", name="code_search", content="x" * 1200),
            ],
            tools=[],
            model=_make_model(),
            options={},
        )

        self.assertIsNotNone(result)
        self.assertEqual(result.tool_message_count, 1)
        self.assertEqual(result.largest_tool_message_name, "code_search")
        self.assertGreater(result.largest_tool_message_tokens, 0)

    def test_counting_auto_prefers_tiktoken_for_openai_compatible_models(self) -> None:
        with patch("openagent.core.token_counting._load_tiktoken_module", return_value=_FakeTiktoken()):
            result = check_context_budget(
                system="You are helpful.",
                messages=[ChatMessage(role="user", content="hello")],
                tools=[],
                model=_make_model(provider_id="openai"),
                options={"context_budget": {"counting": "auto"}},
            )

        self.assertIsNotNone(result)
        self.assertEqual(result.counting_method, "tiktoken")
        self.assertTrue(result.counting_exact)
        self.assertEqual(result.payload_kind, "openai_compatible")

    def test_explicit_input_safety_margin_overrides_guard_ratio(self) -> None:
        result = check_context_budget(
            system="You are helpful.",
            messages=[ChatMessage(role="user", content="hello")],
            tools=[],
            model=_make_model(context_window=4096, max_output=512),
            options={"context_budget": {"guard_ratio": 0.9, "input_safety_margin_tokens": 256}},
        )

        self.assertIsNotNone(result)
        self.assertEqual(result.input_limit_tokens, 4096 - 512 - 256)

    def test_compaction_facade_maps_to_context_budget_options(self) -> None:
        config = load_context_budget_options(
            {
                "compaction": {
                    "auto": False,
                    "prune": False,
                    "reserved": 2048,
                    "mode": "structured_work_state",
                }
            },
            model=_make_model(context_window=8192, max_output=512),
        )

        self.assertEqual(config["strategy"], "error")
        self.assertFalse(config["prune_old_tool_outputs"])
        self.assertEqual(config["input_safety_margin_tokens"], 2048)
        self.assertTrue(config["use_safety_margin_tokens"])
        self.assertEqual(config["compaction_mode"], "structured_work_state")

    def test_context_budget_options_override_compaction_facade(self) -> None:
        config = load_context_budget_options(
            {
                "compaction": {
                    "auto": False,
                    "prune": False,
                    "reserved": 2048,
                },
                "context_budget": {
                    "strategy": "compact",
                    "prune_old_tool_outputs": True,
                    "input_safety_margin_tokens": 128,
                },
            },
            model=_make_model(context_window=8192, max_output=512),
        )

        self.assertEqual(config["strategy"], "compact")
        self.assertTrue(config["prune_old_tool_outputs"])
        self.assertEqual(config["input_safety_margin_tokens"], 128)

    def test_compaction_facade_auto_true_maps_to_auto_strategy(self) -> None:
        config = load_context_budget_options(
            {"compaction": {"auto": True}},
            model=_make_model(context_window=8192, max_output=512),
        )

        self.assertEqual(config["strategy"], "auto")

    def test_invalid_compaction_facade_raises_clear_error(self) -> None:
        with self.assertRaises(ContextBudgetConfigError) as ctx:
            load_context_budget_options(
                {"compaction": {"auto": "yes"}},
                model=_make_model(),
            )

        self.assertIn("compaction.auto must be a bool", str(ctx.exception))

    def test_invalid_compaction_mode_raises_clear_error(self) -> None:
        with self.assertRaises(ContextBudgetConfigError) as ctx:
            load_context_budget_options(
                {"compaction": {"mode": "research_summary"}},
                model=_make_model(),
            )

        self.assertIn("Unsupported context_budget.compaction_mode", str(ctx.exception))

    def test_runtime_options_are_not_forwarded_as_provider_options(self) -> None:
        payload = materialize_openai_compatible_payload(
            system="You are helpful.",
            messages=[ChatMessage(role="user", content="hello")],
            tools=[],
            model=_make_model(provider_id="openai"),
            options={
                "context_budget": {"strategy": "auto"},
                "compaction": {"auto": True},
                "reasoning_effort": "low",
            },
        )

        self.assertEqual(payload["provider_options"], {"reasoning_effort": "low"})
