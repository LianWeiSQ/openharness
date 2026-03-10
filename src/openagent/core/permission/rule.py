from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Any, Callable


class PermissionAction(str, Enum):
    ALLOW = "allow"
    DENY = "deny"
    ASK = "ask"


@dataclass(frozen=True, slots=True)
class PermissionRule:
    tool: str
    action: PermissionAction
    pattern: str | None = None
    condition: Callable[[dict[str, Any]], bool] | None = None

