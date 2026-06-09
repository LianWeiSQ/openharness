from __future__ import annotations

import json
import unittest
from unittest.mock import patch

from openagent.core.message_materializer import materialize_openai_compatible_messages, materialize_openai_compatible_tools
from openagent.core.provider.openai import OpenAILanguageModel, OpenAIProvider, _parse_tool_arguments
from openagent.core.types import ChatMessage, Model, ToolSchema, Usage


class _FakeResponse:
    def __init__(self, lines: list[bytes]) -> None:
        self._lines = lines
        self.status = 200
        self.headers: dict[str, str] = {"Content-Type": "application/json"}

    def __enter__(self):  # noqa: ANN204
        return self

    def __exit__(self, exc_type, exc, tb):  # noqa: ANN001,ANN201
        return False

    def __iter__(self):
        return iter(self._lines)

    def read(self) -> bytes:
        return b"".join(self._lines)


class OpenAIStreamingTests(unittest.IsolatedAsyncioTestCase):
    async def test_provider_uses_timeout_from_environment(self) -> None:
        with patch.dict("os.environ", {"OPENAI_API_KEY": "test", "OPENAI_TIMEOUT_S": "240"}, clear=False):
            provider = OpenAIProvider(base_url="https://example.invalid")
            model = await provider.get_language_model(
                model=Model(
                    id="gpt-test",
                    provider_id="openai",
                    name="gpt-test",
                    context_window=128000,
                    max_output=4096,
                )
            )

        self.assertIsInstance(model, OpenAILanguageModel)
        self.assertEqual(model.timeout_s, 240)

    def test_parse_tool_arguments_recovers_from_repeated_cumulative_snapshot(self) -> None:
        malformed = (
            '{"query":"climate tipping points Arctic ice sheet Amazon rainforest permafrost feedback cascade",'
            '"num_results":8,"timeout":60'
            '{"query":"climate tipping points Arctic ice sheet Amazon rainforest permafrost feedback cascade",'
            '"num_results":8,"timeout":60}'
        )

        parsed = _parse_tool_arguments(malformed)

        self.assertEqual(
            parsed,
            {
                "query": "climate tipping points Arctic ice sheet Amazon rainforest permafrost feedback cascade",
                "num_results": 8,
                "timeout": 60,
            },
        )

    async def test_sse_streaming_parses_text_and_tool_calls_with_host_header(self) -> None:
        chunks = [
            {"choices": [{"index": 0, "delta": {"content": "Hello "}, "finish_reason": None}]},
            {"choices": [{"index": 0, "delta": {"content": "world"}, "finish_reason": None}]},
            {
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "id": "call_1",
                                    "type": "function",
                                    "function": {"name": "ls", "arguments": '{"path":'},
                                }
                            ]
                        },
                        "finish_reason": None,
                    }
                ]
            },
            {
                "choices": [
                    {
                        "index": 0,
                        "delta": {"tool_calls": [{"index": 0, "function": {"arguments": '"."}'}}]},
                        "finish_reason": None,
                    }
                ]
            },
            {
                "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5},
            },
        ]
        sse_lines = [f"data: {json.dumps(chunk, ensure_ascii=False)}\n".encode("utf-8") for chunk in chunks] + [b"data: [DONE]\n"]

        seen_payload: dict[str, object] = {}
        seen_headers: dict[str, str] = {}

        def _fake_urlopen(req, timeout=None):  # noqa: ANN001,ANN201
            nonlocal seen_payload, seen_headers
            body = getattr(req, "data", None) or b"{}"
            seen_payload = json.loads(body.decode("utf-8"))
            seen_headers = {key.lower(): value for key, value in req.header_items()}
            return _FakeResponse(sse_lines)

        model = OpenAILanguageModel(
            api_key="test",
            model_id="glm47",
            base_url="https://gateway.example.test/v1",
            host_header="model-gateway.example.test",
        )
        messages = [
            ChatMessage(role="user", content="show files"),
            ChatMessage(
                role="assistant",
                content="",
                metadata={
                    "tool_calls": [
                        {
                            "id": "prior_call",
                            "type": "function",
                            "function": {"name": "ls", "arguments": '{"path":"."}'},
                        }
                    ]
                },
            ),
            ChatMessage(role="tool", name="ls", tool_call_id="prior_call", content="[Tool result] ls"),
        ]
        tools = [ToolSchema(name="ls", description="List directory", schema={"type": "object", "properties": {"path": {"type": "string"}}})]

        events: list[dict] = []
        with patch("urllib.request.urlopen", new=_fake_urlopen):
            async for ev in model.stream(system="You are helpful.", messages=messages, tools=tools):
                events.append(ev)

        self.assertEqual(seen_payload.get("stream"), True)
        self.assertEqual(seen_payload["messages"], materialize_openai_compatible_messages("You are helpful.", messages))
        self.assertEqual(seen_payload["tools"], materialize_openai_compatible_tools(tools))
        self.assertEqual(seen_headers.get("host"), "model-gateway.example.test")

        self.assertEqual([e["type"] for e in events[:2]], ["text-delta", "text-delta"])
        self.assertEqual(events[0]["text"], "Hello ")
        self.assertEqual(events[1]["text"], "world")

        tool_call = next(e for e in events if e["type"] == "tool-call")
        self.assertEqual(tool_call["call_id"], "call_1")
        self.assertEqual(tool_call["name"], "ls")
        self.assertEqual(tool_call["input"], {"path": "."})

        finish = next(e for e in events if e["type"] == "finish")
        self.assertEqual(finish["finish_reason"], "tool_call")
        self.assertIsInstance(finish["usage"], Usage)
        self.assertEqual(finish["usage"].input_tokens, 3)
        self.assertEqual(finish["usage"].output_tokens, 2)

    async def test_sse_streaming_parses_cumulative_tool_argument_snapshots(self) -> None:
        chunks = [
            {
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "id": "call_1",
                                    "type": "function",
                                    "function": {
                                        "name": "web_search",
                                        "arguments": '{"query":"climate tipping points","num_results":8,"timeout":60',
                                    },
                                }
                            ]
                        },
                        "finish_reason": None,
                    }
                ]
            },
            {
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "function": {
                                        "arguments": '{"query":"climate tipping points","num_results":8,"timeout":60}',
                                    },
                                }
                            ]
                        },
                        "finish_reason": None,
                    }
                ]
            },
            {
                "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5},
            },
        ]
        sse_lines = [f"data: {json.dumps(chunk, ensure_ascii=False)}\n".encode("utf-8") for chunk in chunks] + [b"data: [DONE]\n"]

        def _fake_urlopen(req, timeout=None):  # noqa: ANN001,ANN201
            del req, timeout
            return _FakeResponse(sse_lines)

        model = OpenAILanguageModel(api_key="test", model_id="glm47", base_url="https://example.invalid")

        events: list[dict] = []
        with patch("urllib.request.urlopen", new=_fake_urlopen):
            async for ev in model.stream(system="You are helpful.", messages=[], tools=[]):
                events.append(ev)

        tool_call = next(e for e in events if e["type"] == "tool-call")
        self.assertEqual(tool_call["call_id"], "call_1")
        self.assertEqual(tool_call["name"], "web_search")
        self.assertEqual(
            tool_call["input"],
            {
                "query": "climate tipping points",
                "num_results": 8,
                "timeout": 60,
            },
        )

    async def test_sse_streaming_supports_structured_content_and_cumulative_message_snapshots(self) -> None:
        chunks = [
            {
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "content": [
                                {"type": "text", "text": "Hel"},
                                {"type": "text", "text": {"value": "lo"}},
                            ]
                        },
                        "finish_reason": None,
                    }
                ]
            },
            {
                "choices": [
                    {
                        "index": 0,
                        "message": {"content": [{"type": "output_text", "text": "Hello world"}]},
                        "finish_reason": None,
                    }
                ]
            },
            {
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
            },
        ]
        sse_lines = [f"data: {json.dumps(chunk, ensure_ascii=False)}\n".encode("utf-8") for chunk in chunks] + [b"data: [DONE]\n"]

        def _fake_urlopen(req, timeout=None):  # noqa: ANN001,ANN201
            del req, timeout
            return _FakeResponse(sse_lines)

        model = OpenAILanguageModel(api_key="test", model_id="glm47", base_url="https://example.invalid")

        events: list[dict] = []
        with patch("urllib.request.urlopen", new=_fake_urlopen):
            async for ev in model.stream(system="You are helpful.", messages=[], tools=[]):
                events.append(ev)

        text_events = [event for event in events if event["type"] == "text-delta"]
        self.assertEqual([event["text"] for event in text_events], ["Hello", " world"])

        finish = next(e for e in events if e["type"] == "finish")
        self.assertEqual(finish["finish_reason"], "stop")
        self.assertIsInstance(finish["usage"], Usage)
        self.assertEqual(finish["usage"].input_tokens, 2)
        self.assertEqual(finish["usage"].output_tokens, 3)

    async def test_responses_api_parses_text_and_tool_calls(self) -> None:
        payload = {
            "output": [
                {
                    "type": "function_call",
                    "call_id": "call_hello",
                    "name": "bash",
                    "arguments": '{"command":"printf hello > hello.txt","timeout":10000}',
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "running"}],
                },
            ],
            "usage": {"input_tokens": 7, "output_tokens": 11},
        }
        seen_payload: dict[str, object] = {}

        def _fake_urlopen(req, timeout=None):  # noqa: ANN001,ANN201
            nonlocal seen_payload
            del timeout
            seen_payload = json.loads((getattr(req, "data", None) or b"{}").decode("utf-8"))
            return _FakeResponse([json.dumps(payload).encode("utf-8")])

        model = OpenAILanguageModel(
            api_key="test",
            model_id="gpt-5.4",
            base_url="https://example.invalid",
            wire_api="responses",
            reasoning_effort="xhigh",
            disable_response_storage=True,
        )
        messages = [
            ChatMessage(role="user", content="create hello"),
            ChatMessage(
                role="assistant",
                content="",
                metadata={
                    "tool_calls": [
                        {
                            "id": "prior_call",
                            "type": "function",
                            "function": {"name": "bash", "arguments": '{"command":"ls"}'},
                        }
                    ]
                },
            ),
            ChatMessage(role="tool", name="bash", tool_call_id="prior_call", content="[Tool result] bash"),
        ]
        tools = [
            ToolSchema(
                name="bash",
                description="Run a shell command",
                schema={"type": "object", "properties": {"command": {"type": "string"}}},
            )
        ]

        events: list[dict] = []
        with patch("urllib.request.urlopen", new=_fake_urlopen):
            async for ev in model.stream(system="Use tools.", messages=messages, tools=tools):
                events.append(ev)

        self.assertEqual(seen_payload["instructions"], "Use tools.")
        self.assertEqual(seen_payload["store"], False)
        self.assertEqual(seen_payload["reasoning"], {"effort": "xhigh"})
        self.assertEqual(seen_payload["tools"][0]["type"], "function")
        self.assertEqual(seen_payload["tools"][0]["name"], "bash")
        self.assertEqual(
            seen_payload["input"],
            [
                {"role": "user", "content": "create hello"},
                {"type": "function_call", "call_id": "prior_call", "name": "bash", "arguments": '{"command":"ls"}'},
                {"type": "function_call_output", "call_id": "prior_call", "output": "[Tool result] bash"},
            ],
        )
        self.assertEqual(events[0]["type"], "text-delta")
        self.assertEqual(events[0]["text"], "running")
        tool_call = next(e for e in events if e["type"] == "tool-call")
        self.assertEqual(tool_call["call_id"], "call_hello")
        self.assertEqual(tool_call["name"], "bash")
        self.assertEqual(tool_call["input"], {"command": "printf hello > hello.txt", "timeout": 10000})
        finish = events[-1]
        self.assertEqual(finish["type"], "finish")
        self.assertEqual(finish["finish_reason"], "tool_call")
        self.assertEqual(finish["usage"].input_tokens, 7)
        self.assertEqual(finish["usage"].output_tokens, 11)
