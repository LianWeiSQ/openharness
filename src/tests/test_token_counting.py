from __future__ import annotations

import unittest
from unittest.mock import patch

from openagent.core.message_materializer import (
    materialize_openai_compatible_messages,
    materialize_openai_compatible_tools,
    materialize_payload,
)
from openagent.core.token_counting import count_materialized_payload
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


class TokenCountingTests(unittest.TestCase):
    def _make_model(self, provider_id: str = "openai") -> Model:
        return Model(
            id="test-model",
            provider_id=provider_id,
            name="Test Model",
            context_window=8192,
            max_output=1024,
        )

    def test_openai_compatible_payload_uses_tiktoken_when_available(self) -> None:
        materialized = materialize_payload(
            system="You are helpful.",
            messages=[ChatMessage(role="user", content="hello")],
            tools=[ToolSchema(name="ls", description="List files")],
            model=self._make_model("openai"),
            options=None,
        )

        with patch("openagent.core.token_counting._load_tiktoken_module", return_value=_FakeTiktoken()):
            result = count_materialized_payload(
                materialized,
                model=self._make_model("openai"),
                options=None,
                counting="auto",
                bytes_per_token=4,
            )

        self.assertEqual(result.method, "tiktoken")
        self.assertTrue(result.exact)
        self.assertEqual(result.encoding_name, "fake-encoding")
        self.assertGreater(result.tokens, 0)

    def test_auto_falls_back_to_heuristic_when_tiktoken_is_missing(self) -> None:
        materialized = materialize_payload(
            system=None,
            messages=[ChatMessage(role="user", content="hello")],
            tools=[],
            model=self._make_model("openai"),
            options=None,
        )

        with patch("openagent.core.token_counting._load_tiktoken_module", return_value=None):
            result = count_materialized_payload(
                materialized,
                model=self._make_model("openai"),
                options=None,
                counting="auto",
                bytes_per_token=4,
            )

        self.assertEqual(result.method, "heuristic")
        self.assertFalse(result.exact)
        self.assertGreater(result.tokens, 0)

    def test_materializer_and_counter_share_the_same_openai_shape(self) -> None:
        messages = [
            ChatMessage(role="user", content="hello"),
            ChatMessage(
                role="assistant",
                content="",
                metadata={
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "ls", "arguments": '{"path":"."}'},
                        }
                    ]
                },
            ),
        ]
        tools = [ToolSchema(name="ls", description="List files")]
        materialized = materialize_payload(
            system="sys",
            messages=messages,
            tools=tools,
            model=self._make_model("openai"),
            options={"top_p": 0.1},
        )

        self.assertEqual(materialized.payload_kind, "openai_compatible")
        self.assertEqual(materialized.payload["messages"], materialize_openai_compatible_messages("sys", messages))
        self.assertEqual(materialized.payload["tools"], materialize_openai_compatible_tools(tools))
        self.assertEqual(materialized.payload["provider_options"], {"top_p": 0.1})
