from __future__ import annotations

import asyncio
import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Callable

from ..types import ChatMessage, Model, ModelCapabilities, ToolSchema, Usage
from .base import LanguageModel, ProviderBase


def _post_json(*, url: str, headers: dict[str, str], payload: dict[str, Any], timeout_s: float) -> dict[str, Any]:
    data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(url=url, data=data, method="POST")
    for key, value in headers.items():
        req.add_header(key, value)
    try:
        with urllib.request.urlopen(req, timeout=timeout_s) as resp:
            raw = resp.read().decode("utf-8", errors="replace")
            return json.loads(raw)
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"OpenAI-compatible HTTP {exc.code}: {raw}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"OpenAI-compatible request failed: {exc}") from exc


def _to_openai_tools(tools: list[ToolSchema]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for tool in tools:
        out.append(
            {
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.schema or {"type": "object", "properties": {}},
                },
            }
        )
    return out


def _provider_options(options: dict[str, Any] | None) -> dict[str, Any]:
    if not isinstance(options, dict):
        return {}
    return {key: value for key, value in options.items() if key != "context_budget"}


def _usage_from_openai(usage: dict[str, Any] | None) -> Usage:
    usage = usage or {}
    return Usage(
        input_tokens=int(usage.get("prompt_tokens", 0)),
        output_tokens=int(usage.get("completion_tokens", 0)),
        cost=0.0,
    )


def _map_finish_reason(value: Any, *, has_tool_calls: bool) -> str:
    if isinstance(value, str):
        if value in ("stop", "length"):
            return value
        if value in ("tool_calls", "tool_call"):
            return "tool_call"
    if has_tool_calls:
        return "tool_call"
    return "unknown"


def _parse_openai_tool_calls(tool_calls: Any, *, prefix: str) -> list[dict[str, Any]]:
    if not isinstance(tool_calls, list):
        return []
    parsed: list[dict[str, Any]] = []
    for tool_call in tool_calls:
        if not isinstance(tool_call, dict):
            continue
        call_id = str(tool_call.get("id") or tool_call.get("call_id") or "")
        fn = tool_call.get("function") if isinstance(tool_call.get("function"), dict) else {}
        name = str(fn.get("name") or tool_call.get("name") or "")
        arguments = fn.get("arguments")
        input_obj: dict[str, Any] = {}
        if isinstance(arguments, str) and arguments.strip():
            try:
                input_obj = json.loads(arguments)
                if not isinstance(input_obj, dict):
                    input_obj = {"_value": input_obj}
            except json.JSONDecodeError:
                input_obj = {"_raw": arguments}
        if not call_id:
            call_id = f"{prefix}_{len(parsed)}"
        parsed.append({"call_id": call_id, "name": name, "input": input_obj})
    return parsed


def _post_sse(
    *,
    url: str,
    headers: dict[str, str],
    payload: dict[str, Any],
    timeout_s: float,
    on_event: Callable[[dict[str, Any]], None],
) -> None:
    data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(url=url, data=data, method="POST")
    for key, value in headers.items():
        req.add_header(key, value)
    try:
        with urllib.request.urlopen(req, timeout=timeout_s) as resp:
            for raw in resp:
                line = raw.decode("utf-8", errors="replace").strip()
                if not line or not line.startswith("data:"):
                    continue
                data_str = line[len("data:") :].strip()
                if data_str == "[DONE]":
                    break
                try:
                    obj = json.loads(data_str)
                except json.JSONDecodeError:
                    continue
                if isinstance(obj, dict):
                    on_event(obj)
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"OpenAI-compatible HTTP {exc.code}: {raw}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"OpenAI-compatible request failed: {exc}") from exc


@dataclass(slots=True)
class OpenAILanguageModel(LanguageModel):
    api_key: str
    model_id: str
    base_url: str = "https://api.openai.com/v1"
    timeout_s: float = 60.0
    host_header: str | None = None

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
        chat_messages: list[dict[str, Any]] = []
        if system:
            chat_messages.append({"role": "system", "content": system})
        for message in messages:
            item: dict[str, Any] = {"role": message.role, "content": message.content}
            if message.role != "tool" and message.name:
                item["name"] = message.name
            if message.tool_call_id:
                item["tool_call_id"] = message.tool_call_id
            tool_calls = (message.metadata or {}).get("tool_calls")
            if message.role == "assistant" and isinstance(tool_calls, list) and tool_calls:
                item["tool_calls"] = tool_calls
                if not message.content:
                    item["content"] = None
            chat_messages.append(item)

        payload: dict[str, Any] = {
            "model": self.model_id,
            "messages": chat_messages,
            "stream": True,
        }
        if temperature is not None:
            payload["temperature"] = temperature
        if max_output_tokens is not None:
            payload["max_tokens"] = max_output_tokens
        if tools:
            payload["tools"] = _to_openai_tools(tools)
            payload.setdefault("tool_choice", "auto")
        if options:
            payload.update(_provider_options(options))

        url = f"{self.base_url}/chat/completions"
        headers = {
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
            "Authorization": f"Bearer {self.api_key}",
        }
        if self.host_header:
            headers["Host"] = self.host_header

        if not payload.get("stream"):
            data = await asyncio.to_thread(_post_json, url=url, headers=headers, payload=payload, timeout_s=self.timeout_s)
            choices = data.get("choices") or []
            content = ""
            tool_calls: list[dict[str, Any]] = []
            finish_reason_raw: Any = None
            if choices and isinstance(choices, list):
                first = choices[0] or {}
                finish_reason_raw = first.get("finish_reason")
                message = first.get("message") or {}
                content = str(message.get("content") or "")
                tool_calls = _parse_openai_tool_calls(message.get("tool_calls"), prefix="openai_call")

            usage = _usage_from_openai(data.get("usage"))
            if content:
                yield {"type": "text-delta", "text": content}
            for tool_call in tool_calls:
                yield {"type": "tool-call", "call_id": tool_call["call_id"], "name": tool_call["name"], "input": tool_call["input"]}
            yield {
                "type": "finish",
                "finish_reason": _map_finish_reason(finish_reason_raw, has_tool_calls=bool(tool_calls)),
                "usage": usage,
            }
            return

        loop = asyncio.get_running_loop()
        queue: "asyncio.Queue[dict[str, Any] | None]" = asyncio.Queue()
        errors: list[BaseException] = []

        def _put(item: dict[str, Any] | None) -> None:
            loop.call_soon_threadsafe(queue.put_nowait, item)

        def _worker() -> None:
            tool_calls_by_index: dict[int, dict[str, Any]] = {}
            finish_reason_raw: Any = None
            usage_raw: dict[str, Any] | None = None

            def _on_obj(obj: dict[str, Any]) -> None:
                nonlocal finish_reason_raw, usage_raw
                choices = obj.get("choices") or []
                if not isinstance(choices, list) or not choices:
                    return
                choice0 = choices[0] or {}
                delta = choice0.get("delta") or {}

                content = delta.get("content")
                if content:
                    _put({"type": "text-delta", "text": str(content)})

                tool_calls = delta.get("tool_calls") or []
                if isinstance(tool_calls, list):
                    for tool_call in tool_calls:
                        if not isinstance(tool_call, dict):
                            continue
                        idx = int(tool_call.get("index", 0))
                        record = tool_calls_by_index.setdefault(idx, {"id": None, "name": None, "arguments": ""})
                        if tool_call.get("id"):
                            record["id"] = tool_call.get("id")
                        fn = tool_call.get("function") if isinstance(tool_call.get("function"), dict) else {}
                        if fn.get("name"):
                            record["name"] = fn.get("name")
                        if isinstance(fn.get("arguments"), str):
                            record["arguments"] += fn.get("arguments")

                if choice0.get("finish_reason") is not None:
                    finish_reason_raw = choice0.get("finish_reason")
                if isinstance(obj.get("usage"), dict):
                    usage_raw = obj.get("usage")

            try:
                _post_sse(url=url, headers=headers, payload=payload, timeout_s=self.timeout_s, on_event=_on_obj)
            except BaseException as exc:  # noqa: BLE001
                errors.append(exc)

            if errors:
                _put(None)
                return

            tool_calls: list[dict[str, Any]] = []
            for idx in sorted(tool_calls_by_index.keys()):
                record = tool_calls_by_index[idx]
                call_id = str(record.get("id") or f"openai_call_{idx}")
                name = str(record.get("name") or "")
                args_text = str(record.get("arguments") or "")
                input_obj: dict[str, Any] = {}
                if args_text.strip():
                    try:
                        loaded = json.loads(args_text)
                        input_obj = loaded if isinstance(loaded, dict) else {"_value": loaded}
                    except json.JSONDecodeError:
                        input_obj = {"_raw": args_text}
                tool_calls.append({"call_id": call_id, "name": name, "input": input_obj})

            for tool_call in tool_calls:
                _put({"type": "tool-call", "call_id": tool_call["call_id"], "name": tool_call["name"], "input": tool_call["input"]})

            _put(
                {
                    "type": "finish",
                    "finish_reason": _map_finish_reason(finish_reason_raw, has_tool_calls=bool(tool_calls)),
                    "usage": _usage_from_openai(usage_raw),
                }
            )
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


class OpenAIProvider(ProviderBase):
    def __init__(
        self,
        *,
        api_key: str | None = None,
        base_url: str | None = None,
        host_header: str | None = None,
    ) -> None:
        self.api_key = api_key or os.getenv("OPENAI_API_KEY") or ""
        self.base_url = base_url or os.getenv("OPENAI_BASE_URL") or "https://api.openai.com/v1"
        self.host_header = host_header or os.getenv("OPENAI_HOST_HEADER") or None

    async def get_language_model(self, model: Model) -> LanguageModel:
        if not self.api_key:
            raise RuntimeError("Missing OPENAI_API_KEY. Set it before using the OpenAI-compatible provider.")
        return OpenAILanguageModel(
            api_key=self.api_key,
            model_id=model.id,
            base_url=self.base_url,
            host_header=self.host_header,
        )

    async def list_models(self) -> list[Model]:
        default_model = os.getenv("OPENAI_MODEL") or "gpt-4o-mini"
        context_window = _env_int("OPENAI_CONTEXT_WINDOW", 32768)
        max_output = _env_int("OPENAI_MAX_OUTPUT", 4096)
        caps = ModelCapabilities(vision=False, tools=True, streaming=True, reasoning=False)
        return [
            Model(
                id=default_model,
                provider_id="openai",
                name=f"OpenAI Compatible/{default_model}",
                context_window=context_window,
                max_output=max_output,
                capabilities=caps,
            )
        ]

    def get_model_config(self, model: Model) -> dict[str, Any]:
        config: dict[str, Any] = {"base_url": self.base_url}
        if self.host_header:
            config["host_header"] = self.host_header
        return config


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        return int(raw)
    except ValueError:
        return default


