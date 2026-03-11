from __future__ import annotations

"""
DashScope（阿里云通义千问）Provider 适配实现（最小可用 demo 版本）。

目标：
- 让 OpenAgent 的 UniversalAgent 能够“真实调用阿里大模型”完成问答
- 不引入第三方依赖（仅用 Python 标准库 urllib）

实现策略：
- 使用 DashScope 的 OpenAI 兼容接口（compatible-mode）
  - URL: https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions
  - 认证：Authorization: Bearer $DASHSCOPE_API_KEY
- 支持 SSE 真流式输出（stream=true）与 OpenAI-compatible tools/tool-calling
"""

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
    """
    使用标准库发起 HTTP JSON 请求（避免额外依赖）。

    说明：
    - 这里走 DashScope 的 OpenAI 兼容接口（compatible-mode），便于复用通用消息结构
    - 本函数是同步的，异步调用请使用 `asyncio.to_thread(...)`
    """

    data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(url=url, data=data, method="POST")
    for k, v in headers.items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout_s) as resp:
            raw = resp.read().decode("utf-8", errors="replace")
            return json.loads(raw)
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"DashScope HTTP {e.code}: {raw}") from e
    except urllib.error.URLError as e:
        raise RuntimeError(f"DashScope request failed: {e}") from e


def _to_openai_tools(tools: list[ToolSchema]) -> list[dict[str, Any]]:
    """
    将 OpenAgent 的 ToolSchema 转成 OpenAI-compatible tools 格式。

    OpenAI Chat Completions tools 结构示例：
    {
      "type": "function",
      "function": {"name": "...", "description": "...", "parameters": {...}}
    }
    """

    out: list[dict[str, Any]] = []
    for t in tools:
        out.append(
            {
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    # schema 可能为空；按 OpenAI 约定 parameters 至少是一个 object
                    "parameters": t.schema or {"type": "object", "properties": {}},
                },
            }
        )
    return out


def _usage_from_openai(usage: dict[str, Any] | None) -> Usage:
    """
    OpenAI-compatible usage：prompt_tokens / completion_tokens / total_tokens
    OpenAgent Usage：input_tokens / output_tokens / cost
    """

    usage = usage or {}
    return Usage(
        input_tokens=int(usage.get("prompt_tokens", 0)),
        output_tokens=int(usage.get("completion_tokens", 0)),
        cost=0.0,
    )


def _map_finish_reason(value: Any, *, has_tool_calls: bool) -> str:
    """
    将 provider 的 finish_reason 映射到 OpenAgent 的 FinishReason（stop/tool_call/length/...）。
    """

    if isinstance(value, str):
        if value in ("stop", "length"):
            return value
        if value in ("tool_calls", "tool_call"):
            return "tool_call"
    if has_tool_calls:
        return "tool_call"
    return "unknown"


def _parse_openai_tool_calls(tool_calls: Any) -> list[dict[str, Any]]:
    """
    解析 OpenAI-compatible tool_calls 列表，返回统一结构：
    [{"call_id": "...", "name": "...", "input": {...}}, ...]
    """

    if not isinstance(tool_calls, list):
        return []
    parsed: list[dict[str, Any]] = []
    for tc in tool_calls:
        if not isinstance(tc, dict):
            continue
        call_id = str(tc.get("id") or tc.get("call_id") or "")
        fn = tc.get("function") if isinstance(tc.get("function"), dict) else {}
        name = str(fn.get("name") or tc.get("name") or "")
        arguments = fn.get("arguments")
        input_obj: dict[str, Any] = {}
        if isinstance(arguments, str) and arguments.strip():
            try:
                input_obj = json.loads(arguments)
                if not isinstance(input_obj, dict):
                    input_obj = {"_value": input_obj}
            except json.JSONDecodeError:
                # 某些模型可能输出不完整 JSON，这里兜底保留原始文本，避免直接崩溃
                input_obj = {"_raw": arguments}
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
    """
    使用标准库读取 SSE 流（兼容 OpenAI Chat Completions stream=true 的返回）。

    中文说明：
    - urllib 是阻塞 IO，这个函数必须在 to_thread 里跑
    - on_event 用于把解析后的“统一事件”回传给 async 层（通过 Queue）
    """

    data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(url=url, data=data, method="POST")
    for k, v in headers.items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout_s) as resp:
            for raw in resp:
                line = raw.decode("utf-8", errors="replace").strip()
                if not line:
                    continue
                if not line.startswith("data:"):
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
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"DashScope HTTP {e.code}: {raw}") from e
    except urllib.error.URLError as e:
        raise RuntimeError(f"DashScope request failed: {e}") from e


@dataclass(slots=True)
class DashScopeLanguageModel(LanguageModel):
    """
    阿里云 DashScope（通义千问）语言模型适配器。

    目前策略：
    - 支持 stream=true 的 SSE 真流式输出
    - 支持 tools/tool-calling：当上层传入 tools 时，会把它们转换为 OpenAI-compatible tools
    - 仍保持 OpenAgent 内部统一事件形态（text-delta / tool-call / finish）
    """

    api_key: str
    model_id: str
    base_url: str = "https://dashscope.aliyuncs.com/compatible-mode/v1"
    timeout_s: float = 60.0

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
        # 将 OpenAgent 的消息结构转换为 OpenAI 兼容结构（Chat Completions）
        chat_messages: list[dict[str, Any]] = []
        if system:
            chat_messages.append({"role": "system", "content": system})
        for m in messages:
            # DashScope compatible-mode 支持 system/user/assistant/tool 等 role
            msg: dict[str, Any] = {"role": m.role, "content": m.content}
            if m.role != "tool" and m.name:
                msg["name"] = m.name
            if m.tool_call_id:
                msg["tool_call_id"] = m.tool_call_id
            # OpenAI-compatible：assistant(tool_calls) 需要把 tool_calls 放在消息上
            tool_calls = (m.metadata or {}).get("tool_calls")
            if m.role == "assistant" and isinstance(tool_calls, list) and tool_calls:
                msg["tool_calls"] = tool_calls
                # tool_calls 场景 content 可以为 null（避免某些兼容网关对空字符串敏感）
                if not m.content:
                    msg["content"] = None
            chat_messages.append(msg)

        # 默认启用 stream=true；允许调用方通过 options 覆盖（例如 {"stream": False}）
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
            # 默认让模型自行决定是否调用工具
            payload.setdefault("tool_choice", "auto")
        if options:
            # 允许调用方透传一些兼容字段（如 top_p 等）
            payload.update(options)

        url = f"{self.base_url}/chat/completions"
        headers = {
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
            "Authorization": f"Bearer {self.api_key}",
        }

        if not payload.get("stream"):
            # 非流式：一次性拿到完整 JSON
            data = await asyncio.to_thread(_post_json, url=url, headers=headers, payload=payload, timeout_s=self.timeout_s)

            # OpenAI 兼容返回：choices[0].message.content / .tool_calls
            choices = data.get("choices") or []
            content = ""
            tool_calls: list[dict[str, Any]] = []
            finish_reason_raw: Any = None
            if choices and isinstance(choices, list):
                first = choices[0] or {}
                finish_reason_raw = first.get("finish_reason")
                msg = first.get("message") or {}
                content = str(msg.get("content") or "")
                tool_calls = _parse_openai_tool_calls(msg.get("tool_calls"))

            u = _usage_from_openai(data.get("usage"))

            # 以“流事件”形式返回：text-delta（一次） + tool-call（可选） + finish
            if content:
                yield {"type": "text-delta", "text": content}
            for tc in tool_calls:
                yield {"type": "tool-call", "call_id": tc["call_id"], "name": tc["name"], "input": tc["input"]}
            yield {"type": "finish", "finish_reason": _map_finish_reason(finish_reason_raw, has_tool_calls=bool(tool_calls)), "usage": u}
            return

        # SSE 真流式：边读边产出 text-delta，tool_calls 需要聚合后再发出 tool-call
        loop = asyncio.get_running_loop()
        q: "asyncio.Queue[dict[str, Any] | None]" = asyncio.Queue()
        errors: list[BaseException] = []

        def _put(item: dict[str, Any] | None) -> None:
            loop.call_soon_threadsafe(q.put_nowait, item)

        def _worker() -> None:
            tool_calls_by_index: dict[int, dict[str, Any]] = {}
            finish_reason_raw: Any = None
            usage_raw: dict[str, Any] | None = None

            def _on_obj(obj: dict[str, Any]) -> None:
                nonlocal finish_reason_raw, usage_raw
                choices = obj.get("choices") or []
                if not isinstance(choices, list) or not choices:
                    return
                c0 = choices[0] or {}
                delta = c0.get("delta") or {}

                # 1) 文本流式增量
                content = delta.get("content")
                if content:
                    _put({"type": "text-delta", "text": str(content)})

                # 2) 工具调用增量（arguments 可能会分片，需要按 index 聚合）
                tcs = delta.get("tool_calls") or []
                if isinstance(tcs, list):
                    for tc in tcs:
                        if not isinstance(tc, dict):
                            continue
                        idx = int(tc.get("index", 0))
                        rec = tool_calls_by_index.setdefault(idx, {"id": None, "name": None, "arguments": ""})
                        if tc.get("id"):
                            rec["id"] = tc.get("id")
                        fn = tc.get("function") if isinstance(tc.get("function"), dict) else {}
                        if fn.get("name"):
                            rec["name"] = fn.get("name")
                        if isinstance(fn.get("arguments"), str):
                            rec["arguments"] += fn.get("arguments")

                # 3) finish_reason / usage（不同兼容网关可能出现在最后一个 chunk）
                if c0.get("finish_reason") is not None:
                    finish_reason_raw = c0.get("finish_reason")
                if isinstance(obj.get("usage"), dict):
                    usage_raw = obj.get("usage")

            try:
                _post_sse(url=url, headers=headers, payload=payload, timeout_s=self.timeout_s, on_event=_on_obj)
            except BaseException as e:  # noqa: BLE001
                errors.append(e)

            if errors:
                # 发生异常时不产出 finish/tool-call，让上层走 retry/error 流程
                _put(None)
                return

            # 结束时统一发出 tool-call（如果有），再发 finish
            tool_calls: list[dict[str, Any]] = []
            for idx in sorted(tool_calls_by_index.keys()):
                rec = tool_calls_by_index[idx]
                call_id = str(rec.get("id") or f"dashscope_call_{idx}")
                name = str(rec.get("name") or "")
                args_text = str(rec.get("arguments") or "")
                input_obj: dict[str, Any] = {}
                if args_text.strip():
                    try:
                        loaded = json.loads(args_text)
                        input_obj = loaded if isinstance(loaded, dict) else {"_value": loaded}
                    except json.JSONDecodeError:
                        input_obj = {"_raw": args_text}
                tool_calls.append({"call_id": call_id, "name": name, "input": input_obj})

            for tc in tool_calls:
                _put({"type": "tool-call", "call_id": tc["call_id"], "name": tc["name"], "input": tc["input"]})

            _put(
                {
                    "type": "finish",
                    "finish_reason": _map_finish_reason(finish_reason_raw, has_tool_calls=bool(tool_calls)),
                    "usage": _usage_from_openai(usage_raw),
                }
            )
            _put(None)

        # 注意：worker 里是阻塞 IO，所以放到线程池里跑；本 async generator 负责把队列里的事件逐个 yield 出去
        worker_task = asyncio.create_task(asyncio.to_thread(_worker))
        try:
            while True:
                item = await q.get()
                if item is None:
                    break
                yield item
        finally:
            await worker_task
        if errors:
            raise errors[0]


class DashScopeProvider(ProviderBase):
    """
    DashScope Provider（最小实现）。

    说明：
    - `Agent.md` 定义了 ProviderManager/ProviderBase 接口，这里提供一个可用实现
    - `list_models()` 仅提供常用模型列表（避免依赖网络动态拉取）
    """

    def __init__(self, *, api_key: str | None = None, base_url: str | None = None) -> None:
        # 优先使用显式传入的 api_key，其次读取环境变量 DASHSCOPE_API_KEY
        self.api_key = api_key or os.getenv("DASHSCOPE_API_KEY") or ""
        # 支持自定义网关/代理（例如企业内网转发）
        self.base_url = base_url or os.getenv("DASHSCOPE_BASE_URL") or "https://dashscope.aliyuncs.com/compatible-mode/v1"

    async def get_language_model(self, model: Model) -> LanguageModel:
        if not self.api_key:
            raise RuntimeError("未检测到 DASHSCOPE_API_KEY，请先设置环境变量再运行。")
        return DashScopeLanguageModel(api_key=self.api_key, model_id=model.id, base_url=self.base_url)

    async def list_models(self) -> list[Model]:
        # 注意：上下文窗口等信息可能随服务调整；这里给出 conservative 默认值（用于配置占位）。
        caps = ModelCapabilities(vision=False, tools=True, streaming=True, reasoning=False)
        return [
            Model(id="qwen-turbo", provider_id="dashscope", name="Qwen Turbo", context_window=32768, max_output=2048, capabilities=caps),
            Model(id="qwen-plus", provider_id="dashscope", name="Qwen Plus", context_window=32768, max_output=4096, capabilities=caps),
            Model(id="qwen-max", provider_id="dashscope", name="Qwen Max", context_window=32768, max_output=4096, capabilities=caps),
        ]

    def get_model_config(self, model: Model) -> dict[str, Any]:
        return {"base_url": self.base_url}
