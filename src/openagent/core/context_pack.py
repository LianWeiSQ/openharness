from __future__ import annotations

import json
from dataclasses import dataclass, field, replace
from typing import Any, Iterable, Literal, Sequence

from .context_budget import DEFAULT_BYTES_PER_TOKEN
from .context_messages import get_context_compaction
from .session.todo import TodoItem
from .types import ChatMessage

ContextItemKind = Literal[
    "runtime",
    "work_state",
    "todo",
    "message",
    "tool_result",
    "instruction",
    "file",
    "skill",
    "mcp",
    "sandbox",
    "diagnostic",
]


@dataclass(frozen=True, slots=True)
class ContextItem:
    id: str
    kind: ContextItemKind
    source: str
    content: str
    priority: int
    token_estimate: int = 0
    pinned: bool = False
    stable_prefix: bool = False
    ttl_turns: int | None = None
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class ContextPackTraceEntry:
    item_id: str
    kind: str
    source: str
    priority: int
    pinned: bool
    stable_prefix: bool
    token_estimate: int
    included: bool
    drop_reason: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "item_id": self.item_id,
            "kind": self.kind,
            "source": self.source,
            "priority": self.priority,
            "pinned": self.pinned,
            "stable_prefix": self.stable_prefix,
            "token_estimate": self.token_estimate,
            "included": self.included,
            "drop_reason": self.drop_reason,
        }


@dataclass(frozen=True, slots=True)
class ContextPack:
    messages: list[ChatMessage]
    items: list[ContextItem]
    trace: list[ContextPackTraceEntry]
    estimated_input_tokens: int

    def trace_dicts(self) -> list[dict[str, Any]]:
        return [entry.to_dict() for entry in self.trace]


@dataclass(frozen=True, slots=True)
class ContextPackBuildOptions:
    token_budget: int | None = None
    bytes_per_token: int = DEFAULT_BYTES_PER_TOKEN
    trace_only: bool = True


class ContextPackBuilder:
    """Build a typed, diagnosable context pack from OpenAgent session state.

    The first version is intentionally trace-first: by default it preserves the
    existing model messages and produces item-level diagnostics. Later slices can
    switch selected sources to item-rendered projection without changing the data
    model.
    """

    def __init__(self, options: ContextPackBuildOptions | None = None) -> None:
        self.options = options or ContextPackBuildOptions()

    def build(
        self,
        *,
        messages: Sequence[ChatMessage],
        metadata: dict[str, Any] | None = None,
        todos: Sequence[TodoItem | dict[str, Any]] | None = None,
        runtime_context: str | None = None,
        sandbox_metadata: dict[str, Any] | None = None,
        extra_items: Iterable[ContextItem] | None = None,
    ) -> ContextPack:
        source_messages = list(messages)
        session_metadata = dict(metadata or {})
        items = self.collect_items(
            messages=source_messages,
            metadata=session_metadata,
            todos=todos or [],
            runtime_context=runtime_context,
            sandbox_metadata=sandbox_metadata,
            extra_items=extra_items or [],
        )
        items = self._with_estimates(self._dedupe_items(items))
        trace = self._project(items)
        included_ids = {entry.item_id for entry in trace if entry.included}
        estimated_input_tokens = sum(item.token_estimate for item in items if item.id in included_ids)
        if self.options.trace_only:
            pack_messages = source_messages
        else:
            pack_messages = [self._item_to_message(item) for item in items if item.id in included_ids]
        return ContextPack(
            messages=pack_messages,
            items=items,
            trace=trace,
            estimated_input_tokens=estimated_input_tokens,
        )

    def collect_items(
        self,
        *,
        messages: Sequence[ChatMessage],
        metadata: dict[str, Any],
        todos: Sequence[TodoItem | dict[str, Any]],
        runtime_context: str | None,
        sandbox_metadata: dict[str, Any] | None,
        extra_items: Iterable[ContextItem],
    ) -> list[ContextItem]:
        items: list[ContextItem] = []
        if runtime_context and runtime_context.strip():
            items.append(
                ContextItem(
                    id="runtime:current",
                    kind="runtime",
                    source="runtime",
                    content=runtime_context.strip(),
                    priority=90,
                    pinned=True,
                    metadata={"synthetic": True},
                )
            )

        work_state = self._work_state_item(metadata=metadata, message_count=len(messages))
        if work_state is not None:
            items.append(work_state)

        sandbox_item = self._sandbox_item(sandbox_metadata or _metadata_dict(metadata.get("execution")))
        if sandbox_item is not None:
            items.append(sandbox_item)

        items.extend(self._todo_items(todos))
        items.extend(self._message_items(messages))
        items.extend(extra_items)
        return items

    def _with_estimates(self, items: list[ContextItem]) -> list[ContextItem]:
        return [
            item
            if item.token_estimate > 0
            else replace(item, token_estimate=estimate_text_tokens(item.content, bytes_per_token=self.options.bytes_per_token))
            for item in items
        ]

    def _project(self, items: list[ContextItem]) -> list[ContextPackTraceEntry]:
        budget = self.options.token_budget
        included: set[str] = set()
        dropped: dict[str, str] = {}
        used = 0

        ranked = sorted(enumerate(items), key=lambda pair: (not pair[1].pinned, -pair[1].priority, pair[0]))
        for _index, item in ranked:
            if budget is None or budget <= 0 or item.pinned or used + item.token_estimate <= budget:
                included.add(item.id)
                used += item.token_estimate
                continue
            dropped[item.id] = "budget"

        return [
            ContextPackTraceEntry(
                item_id=item.id,
                kind=item.kind,
                source=item.source,
                priority=item.priority,
                pinned=item.pinned,
                stable_prefix=item.stable_prefix,
                token_estimate=item.token_estimate,
                included=item.id in included,
                drop_reason=None if item.id in included else dropped.get(item.id, "not_selected"),
            )
            for item in items
        ]

    def _dedupe_items(self, items: list[ContextItem]) -> list[ContextItem]:
        by_id: dict[str, ContextItem] = {}
        order: list[str] = []
        for item in items:
            existing = by_id.get(item.id)
            if existing is None:
                by_id[item.id] = item
                order.append(item.id)
                continue
            if _item_rank(item) > _item_rank(existing):
                by_id[item.id] = item
        return [by_id[item_id] for item_id in order]

    def _work_state_item(self, *, metadata: dict[str, Any], message_count: int) -> ContextItem | None:
        compaction = get_context_compaction(metadata, message_count=message_count)
        if compaction is None:
            return None
        return ContextItem(
            id="work_state:context_compaction",
            kind="work_state",
            source="session.metadata.context_compaction",
            content=str(compaction["summary"]),
            priority=95,
            pinned=True,
            metadata={
                "compacted_until": compaction.get("compacted_until"),
                "format": compaction.get("format"),
                "schema_version": compaction.get("schema_version"),
                "source": compaction.get("source"),
            },
        )

    def _sandbox_item(self, execution: dict[str, Any]) -> ContextItem | None:
        if not execution:
            return None
        mode = str(execution.get("mode") or "").strip()
        if not mode or mode == "local":
            return None
        safe_payload = {
            key: execution.get(key)
            for key in ("mode", "sandbox_id", "remote_workdir")
            if execution.get(key) is not None
        }
        return ContextItem(
            id="sandbox:execution",
            kind="sandbox",
            source="session.metadata.execution",
            content="[Sandbox context]\n" + json.dumps(safe_payload, ensure_ascii=False, sort_keys=True),
            priority=85,
            pinned=True,
            stable_prefix=True,
            metadata=safe_payload,
        )

    def _todo_items(self, todos: Sequence[TodoItem | dict[str, Any]]) -> list[ContextItem]:
        if not todos:
            return []
        normalized: list[dict[str, Any]] = []
        for index, todo in enumerate(todos):
            if isinstance(todo, TodoItem):
                payload = todo.to_dict()
            elif isinstance(todo, dict):
                payload = dict(todo)
            else:
                continue
            payload.setdefault("id", f"todo-{index + 1}")
            normalized.append(payload)
        if not normalized:
            return []
        lines = ["[Todos]"]
        for todo in normalized:
            lines.append(
                f"- ({todo.get('status', 'pending')}/{todo.get('priority', 'medium')}) "
                f"{todo.get('content', '')}"
            )
        return [
            ContextItem(
                id="todo:session",
                kind="todo",
                source="session.todos",
                content="\n".join(lines).strip(),
                priority=80,
                metadata={"count": len(normalized)},
            )
        ]

    def _message_items(self, messages: Sequence[ChatMessage]) -> list[ContextItem]:
        result: list[ContextItem] = []
        for index, message in enumerate(messages):
            kind: ContextItemKind = "tool_result" if message.role == "tool" else "message"
            source = f"session.messages[{index}]"
            identifier = message.tool_call_id or f"{message.role}:{index}"
            priority = 50 if kind == "tool_result" else 40
            result.append(
                ContextItem(
                    id=f"{kind}:{identifier}",
                    kind=kind,
                    source=source,
                    content=message.content,
                    priority=priority,
                    metadata={
                        "role": message.role,
                        "name": message.name,
                        "tool_call_id": message.tool_call_id,
                    },
                )
            )
        return result

    def _item_to_message(self, item: ContextItem) -> ChatMessage:
        return ChatMessage(
            role="assistant",
            content=item.content,
            metadata={
                "synthetic_context_item": True,
                "context_item_id": item.id,
                "context_item_kind": item.kind,
                "context_item_source": item.source,
            },
        )


def estimate_text_tokens(text: str, *, bytes_per_token: int = DEFAULT_BYTES_PER_TOKEN) -> int:
    if bytes_per_token <= 0:
        bytes_per_token = DEFAULT_BYTES_PER_TOKEN
    byte_count = len(str(text or "").encode("utf-8"))
    return max((byte_count + bytes_per_token - 1) // bytes_per_token, 1)


def _item_rank(item: ContextItem) -> tuple[int, int, int]:
    return (1 if item.pinned else 0, item.priority, item.token_estimate)


def _metadata_dict(value: Any) -> dict[str, Any]:
    return dict(value) if isinstance(value, dict) else {}


__all__ = [
    "ContextItem",
    "ContextItemKind",
    "ContextPack",
    "ContextPackBuildOptions",
    "ContextPackBuilder",
    "ContextPackTraceEntry",
    "estimate_text_tokens",
]

