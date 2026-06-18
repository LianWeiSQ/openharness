from __future__ import annotations

import asyncio
import json
import os
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from ..types import ChatMessage, Model, ModelCapabilities, ToolSchema, Usage
from .base import LanguageModel, ProviderBase
from .metadata import default_env_mapping, provider_default_model, provider_label

DEFAULT_ANTHROPIC_MODEL = "claude-sonnet-4-5"
DEFAULT_CONTEXT_WINDOW = 200_000
DEFAULT_MAX_OUTPUT = 8192

AnthropicClientFactory = Callable[..., Any]


def _get(value: Any, key: str, default: Any = None) -> Any:
    if isinstance(value, dict):
        return value.get(key, default)
    return getattr(value, key, default)


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        return int(raw)
    except ValueError:
        return default


def _env_float(name: str, default: float) -> float:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        value = float(raw)
    except ValueError:
        return default
    return value if value > 0 else default


def _parse_json_object(value: Any) -> dict[str, Any]:
    if isinstance(value, dict):
        return dict(value)
    if isinstance(value, list):
        return {"_value": value}
    if not isinstance(value, str):
        return {}
    raw = value.strip()
    if not raw:
        return {}
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return {"_raw": value}
    if isinstance(parsed, dict):
        return parsed
    return {"_value": parsed}


def _tool_call_content_block(call: Any) -> dict[str, Any] | None:
    if not isinstance(call, dict):
        return None
    function = call.get("function") if isinstance(call.get("function"), dict) else {}
    name = str(function.get("name") or call.get("name") or "")
    if not name:
        return None
    call_id = str(call.get("id") or call.get("call_id") or call.get("tool_call_id") or "")
    if not call_id:
        return None
    arguments = function.get("arguments", call.get("input", call.get("arguments")))
    return {
        "type": "tool_use",
        "id": call_id,
        "name": name,
        "input": _parse_json_object(arguments),
    }


def _materialize_messages(messages: list[ChatMessage]) -> list[dict[str, Any]]:
    normalized: list[dict[str, Any]] = []
    for message in messages:
        content = str(message.content or "")
        if message.role == "system":
            continue
        if message.role == "tool":
            tool_call_id = str(message.tool_call_id or "")
            if not tool_call_id:
                continue
            normalized.append(
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": content,
                        }
                    ],
                }
            )
            continue

        if message.role == "assistant":
            blocks: list[dict[str, Any]] = []
            if content:
                blocks.append({"type": "text", "text": content})
            tool_calls = (message.metadata or {}).get("tool_calls")
            if isinstance(tool_calls, list):
                for call in tool_calls:
                    block = _tool_call_content_block(call)
                    if block is not None:
                        blocks.append(block)
            if blocks:
                normalized.append({"role": "assistant", "content": blocks})
            continue

        if message.role == "user" and content:
            normalized.append({"role": "user", "content": content})
    return normalized


def _materialize_tools(tools: list[ToolSchema]) -> list[dict[str, Any]]:
    return [
        {
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.schema or {"type": "object", "properties": {}},
        }
        for tool in tools
    ]


def _map_finish_reason(value: Any, *, has_tool_calls: bool) -> str:
    reason = str(value or "").strip()
    if reason == "tool_use":
        return "tool_call"
    if reason in {"end_turn", "stop_sequence", "stop"}:
        return "stop"
    if reason == "max_tokens":
        return "length"
    if has_tool_calls:
        return "tool_call"
    return "unknown"


@dataclass(slots=True)
class _ToolUseState:
    call_id: str
    name: str
    input_value: Any = None
    partial_json: str = ""
    emitted: bool = False

    def to_event(self) -> dict[str, Any]:
        input_value = self.input_value
        if self.partial_json:
            input_value = _parse_json_object(self.partial_json)
        return {
            "type": "tool-call",
            "call_id": self.call_id,
            "name": self.name,
            "input": _parse_json_object(input_value),
        }


@dataclass(slots=True)
class AnthropicLanguageModel(LanguageModel):
    api_key: str
    model_id: str
    base_url: str | None = None
    timeout_s: float = 60.0
    max_output: int = DEFAULT_MAX_OUTPUT
    client_factory: AnthropicClientFactory | None = None

    async def stream(
        self,
        *,
        system: str | None,
        messages: list[ChatMessage],
        tools: list[ToolSchema],
        temperature: float | None = None,
        max_output_tokens: int | None = None,
        options: dict[str, Any] | None = None,
    ):
        payload = self._build_payload(
            system=system,
            messages=messages,
            tools=tools,
            temperature=temperature,
            max_output_tokens=max_output_tokens,
            options=options,
        )
        loop = asyncio.get_running_loop()
        queue: "asyncio.Queue[dict[str, Any] | None]" = asyncio.Queue()
        errors: list[BaseException] = []

        def _put(item: dict[str, Any] | None) -> None:
            loop.call_soon_threadsafe(queue.put_nowait, item)

        def _worker() -> None:
            tool_uses: dict[int, _ToolUseState] = {}
            finish_reason_raw: Any = None
            input_tokens = 0
            output_tokens = 0

            def _emit_tool(index: int) -> None:
                state = tool_uses.get(index)
                if state is None or state.emitted:
                    return
                state.emitted = True
                _put(state.to_event())

            try:
                client = self._create_client()
                stream = client.messages.create(**payload)
                for event in _iter_stream(stream):
                    nonlocal_finish = _get(event, "type", "")
                    if nonlocal_finish == "message_start":
                        usage = _get(_get(event, "message", {}), "usage", {})
                        input_tokens = int(_get(usage, "input_tokens", input_tokens) or 0)
                        continue
                    if nonlocal_finish == "content_block_start":
                        index = int(_get(event, "index", 0) or 0)
                        block = _get(event, "content_block", {})
                        block_type = _get(block, "type", "")
                        if block_type == "text":
                            text = str(_get(block, "text", "") or "")
                            if text:
                                _put({"type": "text-delta", "text": text})
                        elif block_type == "tool_use":
                            tool_uses[index] = _ToolUseState(
                                call_id=str(_get(block, "id", "") or f"toolu_{index}"),
                                name=str(_get(block, "name", "") or ""),
                                input_value=_get(block, "input", None),
                            )
                        continue
                    if nonlocal_finish == "content_block_delta":
                        index = int(_get(event, "index", 0) or 0)
                        delta = _get(event, "delta", {})
                        delta_type = _get(delta, "type", "")
                        if delta_type == "text_delta":
                            text = str(_get(delta, "text", "") or "")
                            if text:
                                _put({"type": "text-delta", "text": text})
                        elif delta_type == "input_json_delta":
                            state = tool_uses.setdefault(
                                index,
                                _ToolUseState(call_id=f"toolu_{index}", name=""),
                            )
                            state.partial_json += str(_get(delta, "partial_json", "") or "")
                        continue
                    if nonlocal_finish == "content_block_stop":
                        _emit_tool(int(_get(event, "index", 0) or 0))
                        continue
                    if nonlocal_finish == "message_delta":
                        delta = _get(event, "delta", {})
                        reason = _get(delta, "stop_reason", None)
                        if reason is not None:
                            finish_reason_raw = reason
                        usage = _get(event, "usage", {})
                        output_tokens = int(_get(usage, "output_tokens", output_tokens) or 0)
                        continue
                    if nonlocal_finish == "message_stop":
                        break
                for index in sorted(tool_uses):
                    _emit_tool(index)
                _put(
                    {
                        "type": "finish",
                        "finish_reason": _map_finish_reason(finish_reason_raw, has_tool_calls=bool(tool_uses)),
                        "usage": Usage(input_tokens=input_tokens, output_tokens=output_tokens, cost=0.0),
                    }
                )
            except BaseException as exc:  # noqa: BLE001
                errors.append(exc)
            finally:
                _put(None)

        worker_task = asyncio.create_task(asyncio.to_thread(_worker))
        try:
            while True:
                item = await queue.get()
                if item is None:
                    break
                yield item
        finally:
            await worker_task
        if errors:
            raise errors[0]

    def _build_payload(
        self,
        *,
        system: str | None,
        messages: list[ChatMessage],
        tools: list[ToolSchema],
        temperature: float | None,
        max_output_tokens: int | None,
        options: dict[str, Any] | None,
    ) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "model": self.model_id,
            "messages": _materialize_messages(messages),
            "max_tokens": int(max_output_tokens or self.max_output or DEFAULT_MAX_OUTPUT),
            "stream": True,
        }
        if system:
            payload["system"] = system
        if tools:
            payload["tools"] = _materialize_tools(tools)
            payload["tool_choice"] = {"type": "auto"}
        if temperature is not None:
            payload["temperature"] = temperature
        if isinstance(options, dict):
            for key, value in options.items():
                if key not in {"context_budget", "compaction", "logging", "observability", "runtime_warnings", "session_store", "trace"}:
                    payload[key] = value
        return payload

    def _create_client(self) -> Any:
        if self.client_factory is not None:
            return self.client_factory(api_key=self.api_key, base_url=self.base_url, timeout=self.timeout_s)
        try:
            import anthropic  # type: ignore[import-not-found]
        except ImportError as exc:
            raise RuntimeError(
                "Anthropic provider requires the optional 'anthropic' package. "
                "Install it to use OPENAGENT_PROVIDER=anthropic."
            ) from exc
        kwargs: dict[str, Any] = {"api_key": self.api_key, "timeout": self.timeout_s}
        if self.base_url:
            kwargs["base_url"] = self.base_url
        return anthropic.Anthropic(**kwargs)


def _iter_stream(stream: Any):
    if hasattr(stream, "__enter__") and hasattr(stream, "__exit__"):
        with stream as active:
            yield from active
        return
    yield from stream


class AnthropicProvider(ProviderBase):
    def __init__(
        self,
        *,
        api_key: str | None = None,
        model_id: str | None = None,
        base_url: str | None = None,
        timeout_s: float | None = None,
        client_factory: AnthropicClientFactory | None = None,
    ) -> None:
        env = default_env_mapping("anthropic")
        self.provider_id = "anthropic"
        self.api_key = api_key or os.getenv(env["api_key"]) or ""
        self.model_id = model_id or os.getenv(env["model"]) or provider_default_model("anthropic") or DEFAULT_ANTHROPIC_MODEL
        self.base_url = base_url or os.getenv(env["base_url"]) or None
        self.timeout_s = timeout_s or _env_float("ANTHROPIC_TIMEOUT_S", 60.0)
        self.client_factory = client_factory
        self.context_window = _env_int("ANTHROPIC_CONTEXT_WINDOW", DEFAULT_CONTEXT_WINDOW)
        self.max_output = _env_int("ANTHROPIC_MAX_OUTPUT", DEFAULT_MAX_OUTPUT)

    async def get_language_model(self, model: Model) -> LanguageModel:
        if not self.api_key:
            raise RuntimeError("Missing ANTHROPIC_API_KEY. Set it before using the Anthropic provider.")
        return AnthropicLanguageModel(
            api_key=self.api_key,
            model_id=model.id,
            base_url=self.base_url,
            timeout_s=self.timeout_s,
            max_output=model.max_output or self.max_output,
            client_factory=self.client_factory,
        )

    async def list_models(self) -> list[Model]:
        caps = ModelCapabilities(vision=True, tools=True, streaming=True, reasoning=True)
        return [
            Model(
                id=self.model_id,
                provider_id=self.provider_id,
                name=f"{provider_label(self.provider_id)}/{self.model_id}",
                context_window=self.context_window,
                max_output=self.max_output,
                capabilities=caps,
            )
        ]

    def get_model_config(self, model: Model) -> dict[str, Any]:
        del model
        config: dict[str, Any] = {
            "api_key_env": default_env_mapping("anthropic")["api_key"],
            "timeout_s": self.timeout_s,
        }
        if self.base_url:
            config["base_url"] = self.base_url
        return config
