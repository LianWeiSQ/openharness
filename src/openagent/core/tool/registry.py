from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Awaitable, Callable

from ..types import ToolSchema


ToolFunc = Callable[[dict[str, Any], dict[str, Any]], Awaitable[str] | str]


@dataclass(frozen=True, slots=True)
class RegisteredTool:
    schema: ToolSchema
    func: ToolFunc

