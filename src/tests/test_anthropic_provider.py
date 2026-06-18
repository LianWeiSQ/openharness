from __future__ import annotations

import unittest
from unittest.mock import patch

from openagent.core.provider.anthropic import AnthropicLanguageModel, AnthropicProvider
from openagent.core.provider.factory import create_provider
from openagent.core.provider.openai import OpenAIProvider
from openagent.core.types import ChatMessage, Model, ToolSchema, Usage


class _FakeMessages:
    def __init__(self, client: "_FakeAnthropicClient") -> None:
        self._client = client

    def create(self, **payload):  # noqa: ANN003,ANN201
        self._client.requests.append(payload)
        return list(self._client.events)


class _FakeAnthropicClient:
    def __init__(self, events: list[dict]) -> None:
        self.events = events
        self.requests: list[dict] = []
        self.messages = _FakeMessages(self)


class AnthropicProviderTests(unittest.IsolatedAsyncioTestCase):
    async def test_streaming_maps_text_tool_finish_and_usage(self) -> None:
        client = _FakeAnthropicClient(
            [
                {"type": "message_start", "message": {"usage": {"input_tokens": 12}}},
                {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello "}},
                {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "world"}},
                {
                    "type": "content_block_start",
                    "index": 1,
                    "content_block": {"type": "tool_use", "id": "toolu_1", "name": "bash", "input": {}},
                },
                {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": '{"command":"ls"'}},
                {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": ',"timeout":10}'}},
                {"type": "content_block_stop", "index": 1},
                {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 7}},
                {"type": "message_stop"},
            ]
        )
        model = AnthropicLanguageModel(
            api_key="test",
            model_id="claude-test",
            client_factory=lambda **_: client,
        )

        events: list[dict] = []
        async for event in model.stream(system="Be direct.", messages=[ChatMessage(role="user", content="hello")], tools=[]):
            events.append(event)

        self.assertEqual([event["text"] for event in events if event["type"] == "text-delta"], ["Hello ", "world"])
        tool_call = next(event for event in events if event["type"] == "tool-call")
        self.assertEqual(tool_call["call_id"], "toolu_1")
        self.assertEqual(tool_call["name"], "bash")
        self.assertEqual(tool_call["input"], {"command": "ls", "timeout": 10})
        finish = events[-1]
        self.assertEqual(finish["type"], "finish")
        self.assertEqual(finish["finish_reason"], "tool_call")
        self.assertIsInstance(finish["usage"], Usage)
        self.assertEqual(finish["usage"].input_tokens, 12)
        self.assertEqual(finish["usage"].output_tokens, 7)

    async def test_payload_maps_messages_and_tools_to_anthropic_shapes(self) -> None:
        client = _FakeAnthropicClient(
            [
                {"type": "message_start", "message": {"usage": {"input_tokens": 1}}},
                {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}},
                {"type": "message_stop"},
            ]
        )
        model = AnthropicLanguageModel(
            api_key="test",
            model_id="claude-test",
            client_factory=lambda **_: client,
        )
        messages = [
            ChatMessage(role="user", content="inspect repo"),
            ChatMessage(
                role="assistant",
                content="I'll list files.",
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
            ChatMessage(role="tool", name="ls", tool_call_id="call_1", content="[Tool result] ls"),
        ]
        tools = [
            ToolSchema(
                name="ls",
                description="List directory",
                schema={"type": "object", "properties": {"path": {"type": "string"}}},
            )
        ]

        events: list[dict] = []
        async for event in model.stream(
            system="Use tools.",
            messages=messages,
            tools=tools,
            temperature=0.2,
            max_output_tokens=123,
        ):
            events.append(event)

        payload = client.requests[0]
        self.assertEqual(payload["model"], "claude-test")
        self.assertEqual(payload["system"], "Use tools.")
        self.assertEqual(payload["max_tokens"], 123)
        self.assertEqual(payload["temperature"], 0.2)
        self.assertEqual(payload["tools"][0]["name"], "ls")
        self.assertEqual(payload["tools"][0]["input_schema"], tools[0].schema)
        self.assertEqual(payload["tool_choice"], {"type": "auto"})
        self.assertEqual(payload["messages"][0], {"role": "user", "content": "inspect repo"})
        self.assertEqual(
            payload["messages"][1],
            {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I'll list files."},
                    {"type": "tool_use", "id": "call_1", "name": "ls", "input": {"path": "."}},
                ],
            },
        )
        self.assertEqual(
            payload["messages"][2],
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "[Tool result] ls"}]},
        )
        self.assertEqual(events[-1]["finish_reason"], "stop")

    async def test_missing_api_key_is_clean_error(self) -> None:
        model = Model(
            id="claude-test",
            provider_id="anthropic",
            name="Claude Test",
            context_window=200000,
            max_output=8192,
        )

        with patch.dict("os.environ", {}, clear=True):
            provider = AnthropicProvider(client_factory=lambda **_: _FakeAnthropicClient([]))
            with self.assertRaisesRegex(RuntimeError, "Missing ANTHROPIC_API_KEY"):
                await provider.get_language_model(model)

    async def test_provider_lists_configured_model_without_network(self) -> None:
        with patch.dict(
            "os.environ",
            {
                "ANTHROPIC_API_KEY": "test",
                "ANTHROPIC_MODEL": "claude-custom",
                "ANTHROPIC_CONTEXT_WINDOW": "111",
                "ANTHROPIC_MAX_OUTPUT": "222",
            },
            clear=True,
        ):
            provider = AnthropicProvider(client_factory=lambda **_: _FakeAnthropicClient([]))
            models = await provider.list_models()
            language_model = await provider.get_language_model(models[0])

        self.assertEqual(models[0].provider_id, "anthropic")
        self.assertEqual(models[0].id, "claude-custom")
        self.assertEqual(models[0].context_window, 111)
        self.assertEqual(models[0].max_output, 222)
        self.assertIsInstance(language_model, AnthropicLanguageModel)

    def test_factory_selects_anthropic_and_preserves_openai_compatible_default(self) -> None:
        with patch.dict("os.environ", {"OPENAGENT_PROVIDER": "anthropic", "ANTHROPIC_API_KEY": "test"}, clear=True):
            self.assertIsInstance(create_provider(), AnthropicProvider)
            self.assertIsInstance(create_provider("anthropic"), AnthropicProvider)

        with patch.dict("os.environ", {"OPENAGENT_PROVIDER": "openrouter", "OPENROUTER_API_KEY": "test"}, clear=True):
            provider = create_provider()

        self.assertIsInstance(provider, OpenAIProvider)
        self.assertEqual(provider.provider_id, "openrouter")


if __name__ == "__main__":
    unittest.main()
