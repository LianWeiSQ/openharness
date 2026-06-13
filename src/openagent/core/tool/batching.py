from __future__ import annotations

"""Tool-call batch planning.

The planner mirrors Claude Code's core scheduling shape: keep the model's
tool-call order, group consecutive concurrency-safe calls, and serialize tools
whose side effects are unknown or exclusive.
"""

from dataclasses import dataclass
from typing import Literal

from ..types import ToolCall
from .definition import ToolDefinition, ToolExecutionSchema
from .registry import ToolRegistry

ToolBatchMode = Literal["concurrent", "serial"]


@dataclass(frozen=True)
class ToolBatchItem:
    index: int
    call: ToolCall
    tool: ToolDefinition | None
    execution_schema: ToolExecutionSchema
    reason: str


@dataclass(frozen=True)
class ToolBatch:
    index: int
    mode: ToolBatchMode
    items: tuple[ToolBatchItem, ...]
    batch_group: str
    max_parallelism: int | None = None

    @property
    def tool_call_ids(self) -> tuple[str, ...]:
        return tuple(item.call.call_id for item in self.items)

    @property
    def tool_names(self) -> tuple[str, ...]:
        return tuple(item.call.name for item in self.items)


class ToolBatchPlanner:
    """Partition model-produced tool calls into runtime execution batches."""

    def __init__(self, registry: ToolRegistry) -> None:
        self.registry = registry

    def plan(self, calls: list[ToolCall] | tuple[ToolCall, ...]) -> list[ToolBatch]:
        batches: list[ToolBatch] = []
        current_safe_items: list[ToolBatchItem] = []

        def flush_safe() -> None:
            if not current_safe_items:
                return
            batches.append(_make_batch(index=len(batches), items=tuple(current_safe_items), safe=True))
            current_safe_items.clear()

        for index, call in enumerate(calls):
            item = self._item_for_call(index=index, call=call)
            if _is_concurrency_safe(item.execution_schema):
                prospective_items = [*current_safe_items, item]
                if _safe_batch_over_limit(prospective_items):
                    flush_safe()
                current_safe_items.append(item)
                continue

            flush_safe()
            batches.append(_make_batch(index=len(batches), items=(item,), safe=False))

        flush_safe()
        return batches

    def _item_for_call(self, *, index: int, call: ToolCall) -> ToolBatchItem:
        tool = self.registry.get(call.name)
        if tool is None:
            return ToolBatchItem(
                index=index,
                call=call,
                tool=None,
                execution_schema=ToolExecutionSchema(concurrency="unknown", batch_group="unknown"),
                reason="unknown_tool",
            )

        schema = tool.execution_schema
        if schema.concurrency == "safe":
            reason = "concurrency_safe"
        elif schema.concurrency == "exclusive":
            reason = "exclusive_tool"
        elif schema.concurrency == "keyed":
            reason = "keyed_tool_serialized_in_p0"
        else:
            reason = "unknown_concurrency"

        return ToolBatchItem(
            index=index,
            call=call,
            tool=tool,
            execution_schema=schema,
            reason=reason,
        )


def _is_concurrency_safe(schema: ToolExecutionSchema) -> bool:
    return schema.concurrency == "safe"


def _make_batch(*, index: int, items: tuple[ToolBatchItem, ...], safe: bool) -> ToolBatch:
    mode: ToolBatchMode = "concurrent" if safe and len(items) > 1 else "serial"
    groups = {item.execution_schema.batch_group for item in items}
    max_parallelism_values = [
        item.execution_schema.max_parallelism
        for item in items
        if item.execution_schema.max_parallelism is not None
    ]
    return ToolBatch(
        index=index,
        mode=mode,
        items=items,
        batch_group=next(iter(groups)) if len(groups) == 1 else "mixed",
        max_parallelism=min(max_parallelism_values) if max_parallelism_values else None,
    )


def _safe_batch_over_limit(items: list[ToolBatchItem]) -> bool:
    max_parallelism_values = [
        item.execution_schema.max_parallelism
        for item in items
        if item.execution_schema.max_parallelism is not None
    ]
    if not max_parallelism_values:
        return False
    return len(items) > min(max_parallelism_values)


__all__ = ["ToolBatch", "ToolBatchItem", "ToolBatchMode", "ToolBatchPlanner"]
