from __future__ import annotations

"""
Core tool definition types.

These types keep the tool layer internally consistent:
- `ToolDefinition` is the single source of truth for registration metadata.
- `ToolContext` carries execution-scoped information.
- `ToolOutput` is the tool-layer return type, distinct from loop-level `ToolResult`.
"""

from dataclasses import MISSING, dataclass, field, fields, is_dataclass
from enum import Enum
from pathlib import Path
from types import UnionType
from typing import Any, Awaitable, Callable, Literal, Union, get_args, get_origin, get_type_hints


@dataclass
class ToolContext:
    """Execution context passed to tool implementations."""

    session_id: str
    session_root: Path
    call_id: str
    extra: dict[str, Any] = field(default_factory=dict)

    def metadata(self, **kwargs: Any) -> None:
        """Store extra metadata for downstream consumers."""

        self.extra.update(kwargs)


@dataclass
class ToolOutput:
    """Structured tool output returned by `ToolDefinition.execute`."""

    title: str
    output: str
    metadata: dict[str, Any] = field(default_factory=dict)
    truncated: bool = False
    attachments: list[Any] = field(default_factory=list)
    error: str | None = None


@dataclass
class ToolDefinition:
    """Complete tool definition stored in the registry."""

    id: str
    description: str
    parameters: type
    execute: Callable[[Any, ToolContext], Awaitable[ToolOutput] | ToolOutput]
    dangerous: bool = False
    group: str = "default"
    schema_override: dict[str, Any] | None = None

    def parameters_schema(self) -> dict[str, Any]:
        """Return an OpenAI-compatible JSON schema for the tool parameters."""

        if self.schema_override is not None:
            return self.schema_override

        if not is_dataclass(self.parameters):
            return {"type": "object", "properties": {}}

        schema = _type_to_json_schema(self.parameters)
        if schema.get("type") != "object":
            return {"type": "object", "properties": {}}
        return schema


def _strip_optional(tp: Any) -> tuple[Any, bool]:
    """Return `(inner_type, is_optional)` for union-like hints."""

    origin = get_origin(tp)
    if origin is None:
        return tp, False

    if origin in (Union, UnionType):
        args = get_args(tp)
        if args and any(a is type(None) for a in args):  # noqa: E721
            non_none = [a for a in args if a is not type(None)]  # noqa: E721
            inner = non_none[0] if non_none else Any
            return inner, True
    return tp, False


def _type_to_json_schema(tp: Any) -> dict[str, Any]:
    """Map Python type hints to a small JSON Schema subset."""

    inner, _optional = _strip_optional(tp)

    if inner is Any or inner is object:
        return {}

    origin = get_origin(inner)
    args = get_args(inner)

    if origin is Literal:
        literals = list(args)
        if not literals:
            return {"type": "string"}
        schema: dict[str, Any] = {"enum": literals}
        first = literals[0]
        if isinstance(first, bool):
            schema["type"] = "boolean"
        elif isinstance(first, int) and not isinstance(first, bool):
            schema["type"] = "integer"
        elif isinstance(first, float):
            schema["type"] = "number"
        else:
            schema["type"] = "string"
        return schema

    if origin in (list, tuple, set):
        item_tp = args[0] if args else Any
        return {"type": "array", "items": _type_to_json_schema(item_tp)}

    if origin is dict:
        value_tp = args[1] if len(args) > 1 else Any
        return {"type": "object", "additionalProperties": _type_to_json_schema(value_tp)}

    if isinstance(inner, type):
        if issubclass(inner, Enum):
            values = [member.value for member in inner]
            schema: dict[str, Any] = {"enum": values}
            first = values[0] if values else ""
            if isinstance(first, bool):
                schema["type"] = "boolean"
            elif isinstance(first, int) and not isinstance(first, bool):
                schema["type"] = "integer"
            elif isinstance(first, float):
                schema["type"] = "number"
            else:
                schema["type"] = "string"
            return schema

        if is_dataclass(inner):
            return _dataclass_to_json_schema(inner)

    type_map = {
        str: "string",
        int: "integer",
        float: "number",
        bool: "boolean",
    }
    if inner in type_map:
        return {"type": type_map[inner]}

    return {"type": "string"}



def _dataclass_to_json_schema(cls: type) -> dict[str, Any]:
    properties: dict[str, Any] = {}
    required: list[str] = []

    try:
        hints = get_type_hints(cls, include_extras=True)
    except Exception:  # noqa: BLE001
        hints = {}

    for f in fields(cls):
        type_hint = hints.get(f.name, f.type)
        prop = _type_to_json_schema(type_hint)
        if f.metadata.get("description"):
            prop["description"] = f.metadata["description"]
        properties[f.name] = prop

        if f.default is MISSING and f.default_factory is MISSING:  # type: ignore[comparison-overlap]
            required.append(f.name)

    schema: dict[str, Any] = {"type": "object", "properties": properties}
    if required:
        schema["required"] = required
    return schema


ToolParameters = Any
ToolExecuteFunc = Callable[[ToolParameters, ToolContext], Awaitable[ToolOutput] | ToolOutput]

# Backward-compatible aliases.
ToolInfo = ToolDefinition
ToolResult = ToolOutput
