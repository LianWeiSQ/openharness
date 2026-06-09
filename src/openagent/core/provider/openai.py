from __future__ import annotations

import asyncio
import html
import json
import os
import re
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Callable

from ..message_materializer import RUNTIME_OPTION_KEYS, materialize_openai_compatible_payload
from ..types import Model, ModelCapabilities, ToolSchema, Usage
from .base import LanguageModel, ProviderBase

HTTP_ERROR_BODY_PREVIEW_CHARS = 800


def _extract_text_content(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return "".join(part for item in value if (part := _extract_text_content(item)))
    if isinstance(value, dict):
        text_value = value.get("text")
        if isinstance(text_value, str):
            return text_value
        if isinstance(text_value, dict):
            nested = text_value.get("value")
            if isinstance(nested, str):
                return nested

        delta_value = value.get("delta")
        if isinstance(delta_value, str):
            return delta_value
        if isinstance(delta_value, (dict, list)):
            nested_delta = _extract_text_content(delta_value)
            if nested_delta:
                return nested_delta

        for key in ("content", "value", "output_text"):
            nested = value.get(key)
            if isinstance(nested, (str, dict, list)):
                extracted = _extract_text_content(nested)
                if extracted:
                    return extracted
        return ""
    return ""


def _extract_choice_text(choice: dict[str, Any]) -> str:
    delta = choice.get("delta")
    if isinstance(delta, dict):
        extracted = _extract_text_content(delta)
        if extracted:
            return extracted

    message = choice.get("message")
    if isinstance(message, dict):
        extracted = _extract_text_content(message)
        if extracted:
            return extracted

    return ""


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


def _http_error_message(exc: urllib.error.HTTPError) -> str:
    raw = exc.read().decode("utf-8", errors="replace")
    headers = exc.headers or {}
    content_type = str(headers.get("Content-Type") or headers.get("content-type") or "")
    return f"OpenAI-compatible HTTP {exc.code}: {_summarize_http_error_body(raw, content_type)}"


def _summarize_http_error_body(raw: str, content_type: str) -> str:
    text = raw or ""
    lower_type = content_type.lower()
    stripped = text.lstrip()
    looks_like_html = "text/html" in lower_type or stripped.lower().startswith(("<!doctype html", "<html"))
    if looks_like_html:
        title = _extract_html_title(text)
        suffix = f": {title}" if title else ""
        return f"upstream returned HTML error page{suffix}"

    compact = _compact_error_text(text)
    if not compact:
        return "empty response body"
    return _truncate_error_text(compact)


def _extract_html_title(raw: str) -> str:
    match = re.search(r"<title[^>]*>(.*?)</title>", raw, flags=re.IGNORECASE | re.DOTALL)
    if not match:
        return ""
    return _truncate_error_text(_compact_error_text(html.unescape(match.group(1))))


def _compact_error_text(raw: str) -> str:
    return " ".join(str(raw or "").split())


def _truncate_error_text(text: str) -> str:
    if len(text) <= HTTP_ERROR_BODY_PREVIEW_CHARS:
        return text
    return text[:HTTP_ERROR_BODY_PREVIEW_CHARS].rstrip() + "..."


def _post_json(*, url: str, headers: dict[str, str], payload: dict[str, Any], timeout_s: float) -> dict[str, Any]:
    data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(url=url, data=data, method="POST")
    for key, value in headers.items():
        req.add_header(key, value)
    try:
        with urllib.request.urlopen(req, timeout=timeout_s) as resp:
            raw = resp.read().decode("utf-8", errors="replace")
            try:
                return json.loads(raw)
            except json.JSONDecodeError as exc:
                content_type = str(resp.headers.get("Content-Type") or resp.headers.get("content-type") or "")
                preview = _truncate_error_text(_compact_error_text(raw))
                raise RuntimeError(f"OpenAI-compatible response was not JSON: {content_type or 'unknown content type'}: {preview}") from exc
    except urllib.error.HTTPError as exc:
        raise RuntimeError(_http_error_message(exc)) from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"OpenAI-compatible request failed: {exc}") from exc


def _candidate_api_urls(base_url: str, suffix: str) -> list[str]:
    base = base_url.rstrip("/")
    if base.endswith("/v1"):
        return [f"{base}/{suffix.lstrip('/')}"]
    return [f"{base}/{suffix.lstrip('/')}", f"{base}/v1/{suffix.lstrip('/')}"]


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


def _extract_responses_text(value: Any) -> str:
    if not isinstance(value, dict):
        return ""

    output_text = value.get("output_text")
    if isinstance(output_text, str) and output_text:
        return output_text

    texts: list[str] = []
    output = value.get("output")
    if isinstance(output, list):
        for item in output:
            if not isinstance(item, dict):
                continue
            content = item.get("content")
            if isinstance(content, list):
                for part in content:
                    if not isinstance(part, dict):
                        continue
                    extracted = _extract_text_content(part)
                    if extracted:
                        texts.append(extracted)
            else:
                extracted = _extract_text_content(item)
                if extracted:
                    texts.append(extracted)
    if texts:
        return "".join(texts)

    choices = value.get("choices")
    if isinstance(choices, list) and choices and isinstance(choices[0], dict):
        return _extract_choice_text(choices[0])

    return ""


def _materialize_responses_input(system: str | None, messages: list[Any]) -> list[dict[str, Any]]:
    del system
    normalized: list[dict[str, Any]] = []
    for message in messages:
        role = getattr(message, "role", "")
        content = str(getattr(message, "content", "") or "")
        metadata = getattr(message, "metadata", None)
        metadata = metadata if isinstance(metadata, dict) else {}

        if role == "tool":
            call_id = str(getattr(message, "tool_call_id", "") or "")
            if call_id:
                normalized.append({"type": "function_call_output", "call_id": call_id, "output": content})
            continue

        tool_calls = metadata.get("tool_calls")
        if role == "assistant" and isinstance(tool_calls, list) and tool_calls:
            for call in tool_calls:
                item = _responses_function_call_item(call)
                if item is not None:
                    normalized.append(item)
            if content:
                normalized.append({"role": "assistant", "content": content})
            continue

        if role in {"user", "assistant"} and content:
            normalized.append({"role": role, "content": content})
    return normalized


def _responses_function_call_item(call: Any) -> dict[str, Any] | None:
    if not isinstance(call, dict):
        return None
    fn = call.get("function") if isinstance(call.get("function"), dict) else {}
    name = str(fn.get("name") or call.get("name") or "")
    if not name:
        return None
    arguments = fn.get("arguments")
    if not isinstance(arguments, str):
        arguments = json.dumps(arguments if arguments is not None else {}, ensure_ascii=False)
    item: dict[str, Any] = {
        "type": "function_call",
        "call_id": str(call.get("id") or call.get("call_id") or ""),
        "name": name,
        "arguments": arguments,
    }
    if not item["call_id"]:
        item["call_id"] = str(call.get("tool_call_id") or "")
    return item if item["call_id"] else None


def _materialize_responses_tools(tools: list[ToolSchema]) -> list[dict[str, Any]]:
    normalized: list[dict[str, Any]] = []
    for tool in tools:
        normalized.append(
            {
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.schema or {"type": "object", "properties": {}},
            }
        )
    return normalized


def _extract_responses_tool_calls(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, dict):
        return []
    output = value.get("output")
    if not isinstance(output, list):
        return []
    parsed: list[dict[str, Any]] = []
    for item in output:
        if not isinstance(item, dict) or item.get("type") != "function_call":
            continue
        name = str(item.get("name") or "")
        if not name:
            continue
        call_id = str(item.get("call_id") or item.get("id") or f"responses_call_{len(parsed)}")
        parsed.append(
            {
                "call_id": call_id,
                "name": name,
                "input": _parse_tool_arguments(item.get("arguments")),
            }
        )
    return parsed


def _usage_from_responses(usage: dict[str, Any] | None) -> Usage:
    usage = usage or {}
    input_tokens = usage.get("input_tokens", usage.get("prompt_tokens", 0))
    output_tokens = usage.get("output_tokens", usage.get("completion_tokens", 0))
    return Usage(input_tokens=int(input_tokens or 0), output_tokens=int(output_tokens or 0), cost=0.0)


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
        input_obj = _parse_tool_arguments(fn.get("arguments"))
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
        raise RuntimeError(_http_error_message(exc)) from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"OpenAI-compatible request failed: {exc}") from exc


@dataclass(slots=True)
class OpenAILanguageModel(LanguageModel):
    api_key: str
    model_id: str
    base_url: str = "https://api.openai.com/v1"
    timeout_s: float = 60.0
    host_header: str | None = None
    wire_api: str = "chat"
    reasoning_effort: str | None = None
    disable_response_storage: bool = False

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
        if self.wire_api == "responses":
            payload: dict[str, Any] = {
                "model": self.model_id,
                "input": _materialize_responses_input(system, messages),
                "stream": False,
            }
            if system:
                payload["instructions"] = system
            if tools:
                payload["tools"] = _materialize_responses_tools(tools)
                payload["tool_choice"] = "auto"
            if self.disable_response_storage:
                payload["store"] = False
            if self.reasoning_effort:
                payload["reasoning"] = {"effort": self.reasoning_effort}
            if max_output_tokens is not None:
                payload["max_output_tokens"] = max_output_tokens
            provider_options = _provider_options(options)
            if provider_options:
                provider_options.pop("stream", None)
                payload.update(provider_options)

            headers = {
                "Content-Type": "application/json",
                "Accept": "application/json",
                "Authorization": f"Bearer {self.api_key}",
            }
            if self.host_header:
                headers["Host"] = self.host_header
            errors: list[RuntimeError] = []
            data: dict[str, Any] | None = None
            for url in _candidate_api_urls(self.base_url, "responses"):
                try:
                    data = await asyncio.to_thread(_post_json, url=url, headers=headers, payload=payload, timeout_s=self.timeout_s)
                    break
                except RuntimeError as exc:
                    errors.append(exc)
            if data is None:
                raise errors[-1] if errors else RuntimeError("OpenAI-compatible Responses request failed.")
            content = _extract_responses_text(data)
            tool_calls = _extract_responses_tool_calls(data)
            if content:
                yield {"type": "text-delta", "text": content}
            for tool_call in tool_calls:
                yield {"type": "tool-call", "call_id": tool_call["call_id"], "name": tool_call["name"], "input": tool_call["input"]}
            yield {
                "type": "finish",
                "finish_reason": "tool_call" if tool_calls else "stop",
                "usage": _usage_from_responses(data.get("usage")),
            }
            return

        payload = materialize_openai_compatible_payload(
            system=system,
            messages=messages,
            tools=tools,
            model=Model(
                id=self.model_id,
                provider_id="openai",
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
                if isinstance(first, dict):
                    content = _extract_choice_text(first)
                    message = first.get("message") or {}
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
            emitted_text = ""

            def _on_obj(obj: dict[str, Any]) -> None:
                nonlocal finish_reason_raw, usage_raw, emitted_text
                choices = obj.get("choices") or []
                if not isinstance(choices, list) or not choices:
                    return
                choice0 = choices[0] or {}
                delta = choice0.get("delta") or {}

                if isinstance(choice0, dict):
                    text_snapshot = _extract_choice_text(choice0)
                    text_delta, emitted_text = _next_text_delta(text_snapshot, emitted_text)
                    if text_delta:
                        _put({"type": "text-delta", "text": text_delta})

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
                call_id = str(record.get("id") or f"openai_call_{idx}")
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


class OpenAIProvider(ProviderBase):
    def __init__(
        self,
        *,
        api_key: str | None = None,
        base_url: str | None = None,
        host_header: str | None = None,
        wire_api: str | None = None,
        reasoning_effort: str | None = None,
        disable_response_storage: bool = False,
        timeout_s: float | None = None,
    ) -> None:
        self.api_key = api_key or os.getenv("OPENAI_API_KEY") or ""
        self.base_url = base_url or os.getenv("OPENAI_BASE_URL") or "https://api.openai.com/v1"
        if host_header is not None:
            self.host_header = host_header or None
        elif base_url is not None:
            self.host_header = None
        else:
            self.host_header = os.getenv("OPENAI_HOST_HEADER") or None
        self.wire_api = (wire_api or os.getenv("OPENAI_WIRE_API") or "chat").strip().lower()
        if self.wire_api not in {"chat", "responses"}:
            self.wire_api = "chat"
        self.reasoning_effort = reasoning_effort or os.getenv("OPENAI_REASONING_EFFORT") or None
        self.disable_response_storage = disable_response_storage or os.getenv("OPENAI_DISABLE_RESPONSE_STORAGE", "").lower() in {"1", "true", "yes"}
        self.timeout_s = timeout_s or _env_float("OPENAI_TIMEOUT_S", 60.0)

    async def get_language_model(self, model: Model) -> LanguageModel:
        if not self.api_key:
            raise RuntimeError("Missing OPENAI_API_KEY. Set it before using the OpenAI-compatible provider.")
        return OpenAILanguageModel(
            api_key=self.api_key,
            model_id=model.id,
            base_url=self.base_url,
            timeout_s=self.timeout_s,
            host_header=self.host_header,
            wire_api=self.wire_api,
            reasoning_effort=self.reasoning_effort,
            disable_response_storage=self.disable_response_storage,
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
        config: dict[str, Any] = {"base_url": self.base_url, "wire_api": self.wire_api}
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


def _env_float(name: str, default: float) -> float:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        value = float(raw)
    except ValueError:
        return default
    return value if value > 0 else default
