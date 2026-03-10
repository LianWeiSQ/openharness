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
- 为了 demo 简化：不启用 tools/tool-calling；只做对话问答输出
"""

import asyncio
import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any

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


@dataclass(slots=True)
class DashScopeLanguageModel(LanguageModel):
    """
    阿里云 DashScope（通义千问）语言模型适配器。

    目前策略：
    - 只实现“问答对话”能力（不传 tools，不做 tool-calling），用于 demo/最小闭环
    - 以“非真正流式”的方式返回：把完整回答作为一个 `text-delta` 事件发出
      （这样上层依然可以按 Agent.md 的“流事件”消费）
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
        # 将 OpenAgent 的消息结构转换为 OpenAI 兼容结构
        chat_messages: list[dict[str, Any]] = []
        if system:
            chat_messages.append({"role": "system", "content": system})
        for m in messages:
            # DashScope compatible-mode 支持 system/user/assistant/tool 等 role
            msg: dict[str, Any] = {"role": m.role, "content": m.content}
            if m.name:
                msg["name"] = m.name
            if m.tool_call_id:
                msg["tool_call_id"] = m.tool_call_id
            chat_messages.append(msg)

        # 仅做对话问答，不启用工具调用（避免模型输出 tool-calls）
        payload: dict[str, Any] = {
            "model": self.model_id,
            "messages": chat_messages,
            "stream": False,
        }
        if temperature is not None:
            payload["temperature"] = temperature
        if max_output_tokens is not None:
            payload["max_tokens"] = max_output_tokens
        if options:
            # 允许调用方透传一些兼容字段（如 top_p 等）
            payload.update(options)

        url = f"{self.base_url}/chat/completions"
        headers = {
            "Content-Type": "application/json",
            "Authorization": f"Bearer {self.api_key}",
        }

        # 注意：urllib 是阻塞 IO，这里用 to_thread 避免阻塞事件循环
        data = await asyncio.to_thread(_post_json, url=url, headers=headers, payload=payload, timeout_s=self.timeout_s)

        # OpenAI 兼容返回：choices[0].message.content
        choices = data.get("choices") or []
        content = ""
        if choices and isinstance(choices, list):
            msg = (choices[0] or {}).get("message") or {}
            content = str(msg.get("content") or "")

        usage = data.get("usage") or {}
        # usage 字段在兼容接口中一般为：prompt_tokens / completion_tokens / total_tokens
        u = Usage(
            input_tokens=int(usage.get("prompt_tokens", 0)),
            output_tokens=int(usage.get("completion_tokens", 0)),
            cost=0.0,
        )

        # 以“流事件”形式返回：一次性 text-delta + finish
        yield {"type": "text-delta", "id": "dashscope_text", "text": content}
        yield {"type": "finish", "finish_reason": "stop", "usage": u}


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
        caps = ModelCapabilities(vision=False, tools=False, streaming=False, reasoning=False)
        return [
            Model(id="qwen-turbo", provider_id="dashscope", name="Qwen Turbo", context_window=32768, max_output=2048, capabilities=caps),
            Model(id="qwen-plus", provider_id="dashscope", name="Qwen Plus", context_window=32768, max_output=4096, capabilities=caps),
            Model(id="qwen-max", provider_id="dashscope", name="Qwen Max", context_window=32768, max_output=4096, capabilities=caps),
        ]

    def get_model_config(self, model: Model) -> dict[str, Any]:
        return {"base_url": self.base_url}
