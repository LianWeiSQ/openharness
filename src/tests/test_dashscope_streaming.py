from __future__ import annotations

import json
import unittest
from unittest.mock import patch

from openagent.core.message_materializer import materialize_openai_compatible_messages, materialize_openai_compatible_tools
from openagent.core.provider.dashscope import DashScopeLanguageModel
from openagent.core.types import ChatMessage, ToolSchema, Usage


class _FakeResponse:
    def __init__(self, lines: list[bytes]) -> None:
        self._lines = lines

    def __enter__(self):  # noqa: ANN204
        return self

    def __exit__(self, exc_type, exc, tb):  # noqa: ANN001,ANN201
        return False

    def __iter__(self):
        return iter(self._lines)


class DashScopeStreamingTests(unittest.IsolatedAsyncioTestCase):
    async def test_sse_streaming_parses_text_and_tool_calls(self) -> None:
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
        sse_lines = [f"data: {json.dumps(c, ensure_ascii=False)}\n".encode("utf-8") for c in chunks] + [b"data: [DONE]\n"]

        seen_payload: dict[str, object] = {}

        def _fake_urlopen(req, timeout=None):  # noqa: ANN001,ANN201
            nonlocal seen_payload
            body = getattr(req, "data", None) or b"{}"
            seen_payload = json.loads(body.decode("utf-8"))
            return _FakeResponse(sse_lines)

        model = DashScopeLanguageModel(api_key="test", model_id="qwen-test", base_url="https://example.invalid")
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
