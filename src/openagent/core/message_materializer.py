from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from .types import ChatMessage, Model, ToolSchema

OPENAI_COMPATIBLE_PROVIDER_IDS = frozenset({"openai"})
RUNTIME_OPTION_KEYS = frozenset({"context_budget", "compaction", "observability", "logging", "trace", "runtime_warnings"})


@dataclass(frozen=True, slots=True)
class MaterializedPayload:
    payload: dict[str, Any]
    payload_kind: str


def is_openai_compatible_model(model: Model | None) -> bool:
    return model is not None and model.provider_id in OPENAI_COMPATIBLE_PROVIDER_IDS


def materialize_openai_compatible_messages(system: str | None, messages: list[ChatMessage]) -> list[dict[str, Any]]:
    normalized: list[dict[str, Any]] = []
    if system:
        normalized.append({"role": "system", "content": system})
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
        normalized.append(item)
    return normalized


def materialize_openai_compatible_tools(tools: list[ToolSchema]) -> list[dict[str, Any]]:
    normalized: list[dict[str, Any]] = []
    for tool in tools:
        normalized.append(
            {
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.schema or {"type": "object", "properties": {}},
                },
            }
        )
    return normalized


def materialize_openai_compatible_payload(
    *,
    system: str | None,
    messages: list[ChatMessage],
    tools: list[ToolSchema],
    model: Model | None,
    options: dict[str, Any] | None = None,
) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "messages": materialize_openai_compatible_messages(system, messages),
        "tools": materialize_openai_compatible_tools(tools),
    }
    if model is not None:
        payload["model"] = model.id
    if isinstance(options, dict):
        provider_options = {key: value for key, value in options.items() if key not in RUNTIME_OPTION_KEYS}
        if provider_options:
            payload["provider_options"] = provider_options
    return payload


def materialize_payload(
    *,
    system: str | None,
    messages: list[ChatMessage],
    tools: list[ToolSchema],
    model: Model | None,
    options: dict[str, Any] | None = None,
) -> MaterializedPayload:
    if is_openai_compatible_model(model):
        return MaterializedPayload(
            payload=materialize_openai_compatible_payload(
                system=system,
                messages=messages,
                tools=tools,
                model=model,
                options=options,
            ),
            payload_kind="openai_compatible",
        )
    return MaterializedPayload(
        payload=materialize_openai_compatible_payload(
            system=system,
            messages=messages,
            tools=tools,
            model=model,
            options=options,
        ),
        payload_kind="generic",
    )


__all__ = [
    "MaterializedPayload",
    "OPENAI_COMPATIBLE_PROVIDER_IDS",
    "RUNTIME_OPTION_KEYS",
    "is_openai_compatible_model",
    "materialize_openai_compatible_messages",
    "materialize_openai_compatible_payload",
    "materialize_openai_compatible_tools",
    "materialize_payload",
]
