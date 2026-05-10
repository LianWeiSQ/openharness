from __future__ import annotations

import asyncio
import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Callable

from ..message_materializer import RUNTIME_OPTION_KEYS, materialize_openai_compatible_payload
from ..types import Model, ModelCapabilities, ToolSchema, Usage
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
        raise RuntimeError(f"DashScope HTTP {exc.code}: {raw}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"DashScope request failed: {exc}") from exc


def _provider_options(options: dict[str, Any] | None) -> dict[str, Any]:
    if not isinstance(options, dict):
        return {}
    return {key: value for key, value in options.items() if key not in RUNTIME_OPTION_KEYS}


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


def _next_text_delta(raw_text: str, emitted_text: str) -> tuple[str, str]:
    if not raw_text:
        return "", emitted_text
    if raw_text.startswith(emitted_text):
        return raw_text[len(emitted_text) :], raw_text
    if emitted_text.startswith(raw_text):
        return "", emitted_text
    return raw_text, emitted_text + raw_text


def _parse_tool_arguments(arguments: Any) -> dict[str, Any]:
    if isinstance(arguments, dict):
        return dict(arguments)
    if isinstance(arguments, list):
        return {"_value": arguments}
    if not isinstance(arguments, str):
        return {}

    raw_arguments = arguments.strip()
    if not raw_arguments:
        return {}

    parsed = _best_effort_load_json(raw_arguments)
    if parsed is None:
        return {"_raw": arguments}
    if isinstance(parsed, dict):
        return parsed
    return {"_value": parsed}


def _best_effort_load_json(raw_text: str) -> Any | None:
    try:
        return json.loads(raw_text)
    except json.JSONDecodeError:
        pass

    decoder = json.JSONDecoder()
    best_candidate: Any | None = None
    best_score: tuple[int, int, int] | None = None

    for index, char in enumerate(raw_text):
        if char not in "{[":
            continue
        try:
            candidate, end_index = decoder.raw_decode(raw_text, index)
        except json.JSONDecodeError:
            continue

        trailing = raw_text[end_index:].strip()
        score = (1 if not trailing else 0, end_index - index, -index)
        if best_score is None or score > best_score:
            best_candidate = candidate
            best_score = score
            if score[0] == 1:
                break

    return best_candidate


def _parse_openai_tool_calls(tool_calls: Any) -> list[dict[str, Any]]:
    if not isinstance(tool_calls, list):
        return []
    parsed: list[dict[str, Any]] = []
    for tool_call in tool_calls:
        if not isinstance(tool_call, dict):
            continue
        call_id = str(tool_call.get("id") or tool_call.get("call_id") or "")
        fn = tool_call.get("function") if isinstance(tool_call.get("function"), dict) else {}
        name = str(fn.get("name") or tool_call.get("name") or "")
        input_obj = _parse_tool_arguments(fn.get("arguments"))
        if not call_id:
            call_id = f"dashscope_call_{len(parsed)}"
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
        raise RuntimeError(f"DashScope HTTP {exc.code}: {raw}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"DashScope request failed: {exc}") from exc


@dataclass(slots=True)
class DashScopeLanguageModel(LanguageModel):
    api_key: str
    model_id: str
    base_url: str = "https://dashscope.aliyuncs.com/compatible-mode/v1"
    timeout_s: float = 60.0

    async def stream(
        self,
        *,
        system: str | None,
        messages,
        tools: list[ToolSchema],
        temperature: float | None = None,
        max_output_tokens: int | None = None,
        options: dict[str, Any] | None = None,
    ):
        payload = materialize_openai_compatible_payload(
            system=system,
            messages=messages,
            tools=tools,
            model=Model(
                id=self.model_id,
                provider_id="dashscope",
                name=self.model_id,
                context_window=0,
                max_output=0,
            ),
            options=options,
        )
        provider_options = payload.pop("provider_options", None) or _provider_options(options)
        payload["stream"] = True
        if temperature is not None:
            payload["temperature"] = temperature
        if max_output_tokens is not None:
            payload["max_tokens"] = max_output_tokens
        if payload.get("tools"):
            payload.setdefault("tool_choice", "auto")
        if provider_options:
            payload.update(provider_options)

        url = f"{self.base_url}/chat/completions"
        headers = {
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
            "Authorization": f"Bearer {self.api_key}",
        }

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
                tool_calls = _parse_openai_tool_calls(message.get("tool_calls"))

            usage = _usage_from_openai(data.get("usage"))
            if content:
                yield {"type": "text-delta", "text": content}
            for tool_call in tool_calls:
                yield {"type": "tool-call", "call_id": tool_call["call_id"], "name": tool_call["name"], "input": tool_call["input"]}
            yield {"type": "finish", "finish_reason": _map_finish_reason(finish_reason_raw, has_tool_calls=bool(tool_calls)), "usage": usage}
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
                        record = tool_calls_by_index.setdefault(idx, {"id": None, "name": None, "arguments": "", "arguments_emitted": ""})
                        if tool_call.get("id"):
                            record["id"] = tool_call.get("id")
                        fn = tool_call.get("function") if isinstance(tool_call.get("function"), dict) else {}
                        if fn.get("name"):
                            record["name"] = fn.get("name")
                        if isinstance(fn.get("arguments"), str):
                            arguments_delta, arguments_emitted = _next_text_delta(
                                fn.get("arguments"),
                                str(record.get("arguments_emitted") or ""),
                            )
                            if arguments_delta:
                                record["arguments"] += arguments_delta
                            record["arguments_emitted"] = arguments_emitted

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
                call_id = str(record.get("id") or f"dashscope_call_{idx}")
                name = str(record.get("name") or "")
                args_text = str(record.get("arguments") or "")
                input_obj = _parse_tool_arguments(args_text)
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


class DashScopeProvider(ProviderBase):
    def __init__(self, *, api_key: str | None = None, base_url: str | None = None) -> None:
        self.api_key = api_key or os.getenv("DASHSCOPE_API_KEY") or ""
        self.base_url = base_url or os.getenv("DASHSCOPE_BASE_URL") or "https://dashscope.aliyuncs.com/compatible-mode/v1"

    async def get_language_model(self, model: Model) -> LanguageModel:
        if not self.api_key:
            raise RuntimeError("Missing DASHSCOPE_API_KEY. Set it before using the DashScope provider.")
        return DashScopeLanguageModel(api_key=self.api_key, model_id=model.id, base_url=self.base_url)

    async def list_models(self) -> list[Model]:
        caps = ModelCapabilities(vision=False, tools=True, streaming=True, reasoning=False)
        return [
            Model(id="qwen-turbo", provider_id="dashscope", name="Qwen Turbo", context_window=32768, max_output=2048, capabilities=caps),
            Model(id="qwen-plus", provider_id="dashscope", name="Qwen Plus", context_window=32768, max_output=4096, capabilities=caps),
            Model(id="qwen-max", provider_id="dashscope", name="Qwen Max", context_window=32768, max_output=4096, capabilities=caps),
        ]

    def get_model_config(self, model: Model) -> dict[str, Any]:
        return {"base_url": self.base_url}
